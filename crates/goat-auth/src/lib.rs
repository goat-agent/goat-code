use std::{fmt, fs, path::PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

pub const BASE64URL: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::URL_SAFE_NO_PAD;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretString(String);

impl SecretString {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(***)")
    }
}

impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CredentialKey {
    pub provider: String,
    pub account: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    ApiKey,
    OAuth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthTokenSet {
    pub access_token: SecretString,
    pub refresh_token: Option<SecretString>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedCredential {
    ApiKey(SecretString),
    OAuth(OAuthTokenSet),
}

impl ResolvedCredential {
    pub fn kind(&self) -> CredentialKind {
        match self {
            ResolvedCredential::ApiKey(_) => CredentialKind::ApiKey,
            ResolvedCredential::OAuth(_) => CredentialKind::OAuth,
        }
    }

    pub fn bearer(&self) -> &str {
        match self {
            ResolvedCredential::ApiKey(secret) => secret.expose(),
            ResolvedCredential::OAuth(tokens) => tokens.access_token.expose(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredValue {
    ApiKey { secret: SecretString },
    OAuth { tokens: OAuthTokenSet },
}

impl From<ResolvedCredential> for StoredValue {
    fn from(value: ResolvedCredential) -> Self {
        match value {
            ResolvedCredential::ApiKey(secret) => StoredValue::ApiKey { secret },
            ResolvedCredential::OAuth(tokens) => StoredValue::OAuth { tokens },
        }
    }
}

impl From<StoredValue> for ResolvedCredential {
    fn from(value: StoredValue) -> Self {
        match value {
            StoredValue::ApiKey { secret } => ResolvedCredential::ApiKey(secret),
            StoredValue::OAuth { tokens } => ResolvedCredential::OAuth(tokens),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredEntry {
    key: CredentialKey,
    value: StoredValue,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AuthFile {
    credentials: Vec<StoredEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("oauth error: {0}")]
    OAuth(String),
}

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn generate() -> Self {
        let bytes: [u8; 32] = std::array::from_fn(|_| rand::random::<u8>());
        let verifier = BASE64URL.encode(bytes);
        let challenge = BASE64URL.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }
}

pub fn random_state() -> String {
    let bytes: [u8; 32] = std::array::from_fn(|_| rand::random::<u8>());
    BASE64URL.encode(bytes)
}

pub async fn bind_loopback() -> Result<(TcpListener, u16), AuthError> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

pub async fn capture_loopback_code(port: u16, expected_state: &str) -> Result<String, AuthError> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    capture_on(listener, expected_state).await
}

pub async fn capture_on(listener: TcpListener, expected_state: &str) -> Result<String, AuthError> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let mut buf = vec![0u8; 8192];
        let read = stream.read(&mut buf).await?;
        let request = String::from_utf8_lossy(&buf[..read]);
        let target = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1));
        let Some(query) = target.and_then(|path| path.split_once('?')).map(|(_, q)| q) else {
            let _ = stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
            continue;
        };
        let mut code = None;
        let mut state = None;
        for pair in query.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                match key {
                    "code" => code = Some(value.to_owned()),
                    "state" => state = Some(value.to_owned()),
                    _ => {}
                }
            }
        }
        let body = "<html><body>goat-code login complete. You can close this tab.</body></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.flush().await;
        if state.as_deref() != Some(expected_state) {
            return Err(AuthError::OAuth("state mismatch".to_owned()));
        }
        return code.ok_or_else(|| AuthError::OAuth("missing authorization code".to_owned()));
    }
}

#[derive(Clone)]
pub struct CredentialStore {
    path: PathBuf,
}

impl CredentialStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn resolve(
        &self,
        key: &CredentialKey,
        env_var: Option<&str>,
    ) -> Option<ResolvedCredential> {
        if let Some(var) = env_var
            && let Ok(value) = std::env::var(var)
            && !value.is_empty()
        {
            return Some(ResolvedCredential::ApiKey(SecretString::from(value)));
        }
        self.file_get(key)
    }

    pub fn store(&self, key: &CredentialKey, value: ResolvedCredential) -> Result<(), AuthError> {
        self.file_set(key, value)
    }

    pub fn entries(&self) -> Vec<(CredentialKey, CredentialKind)> {
        self.load_file()
            .credentials
            .into_iter()
            .map(|entry| {
                let resolved: ResolvedCredential = entry.value.into();
                (entry.key, resolved.kind())
            })
            .collect()
    }

    pub fn remove(&self, key: &CredentialKey) -> Result<bool, AuthError> {
        let mut file = self.load_file();
        let before = file.credentials.len();
        file.credentials.retain(|entry| &entry.key != key);
        let removed = file.credentials.len() != before;
        if removed {
            self.save_file(&file)?;
        }
        Ok(removed)
    }

    fn load_file(&self) -> AuthFile {
        fs::read_to_string(&self.path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn save_file(&self, file: &AuthFile) -> Result<(), AuthError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.path, serde_json::to_string_pretty(file)?)?;
        Ok(())
    }

    fn file_get(&self, key: &CredentialKey) -> Option<ResolvedCredential> {
        self.load_file()
            .credentials
            .into_iter()
            .find(|entry| &entry.key == key)
            .map(|entry| entry.value.into())
    }

    fn file_set(&self, key: &CredentialKey, value: ResolvedCredential) -> Result<(), AuthError> {
        let mut file = self.load_file();
        let stored = StoredValue::from(value);
        if let Some(entry) = file.credentials.iter_mut().find(|entry| &entry.key == key) {
            entry.value = stored;
        } else {
            file.credentials.push(StoredEntry {
                key: key.clone(),
                value: stored,
            });
        }
        self.save_file(&file)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CredentialKey, CredentialKind, CredentialStore, Pkce, ResolvedCredential, SecretString,
    };

    #[test]
    fn pkce_generates_s256_challenge() {
        use base64::Engine;
        use sha2::{Digest, Sha256};
        let pkce = Pkce::generate();
        assert_eq!(pkce.verifier.len(), 43);
        assert_eq!(
            pkce.challenge,
            super::BASE64URL.encode(Sha256::digest(pkce.verifier.as_bytes()))
        );
    }

    #[test]
    fn secret_string_debug_is_redacted() {
        let secret = SecretString::from("topsecret");
        assert_eq!(format!("{secret:?}"), "SecretString(***)");
        assert_eq!(secret.expose(), "topsecret");
    }

    #[test]
    fn secret_string_serializes_transparently() {
        let secret = SecretString::from("abc");
        assert_eq!(serde_json::to_string(&secret).unwrap(), "\"abc\"");
    }

    #[test]
    fn resolved_credential_kind() {
        let cred = ResolvedCredential::ApiKey(SecretString::from("k"));
        assert_eq!(cred.kind(), CredentialKind::ApiKey);
    }

    #[test]
    fn file_store_roundtrip() {
        let path = std::env::temp_dir().join("goat-auth-file-roundtrip-test.json");
        let _ = std::fs::remove_file(&path);
        let store = CredentialStore::new(path.clone());
        let key = CredentialKey {
            provider: "p".into(),
            account: "a".into(),
        };
        store
            .file_set(&key, ResolvedCredential::ApiKey(SecretString::from("k")))
            .unwrap();
        let got = store.file_get(&key).unwrap();
        assert!(matches!(got, ResolvedCredential::ApiKey(secret) if secret.expose() == "k"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn resolve_prefers_env() {
        let path = std::env::temp_dir().join("goat-auth-env-pref-test.json");
        let _ = std::fs::remove_file(&path);
        let store = CredentialStore::new(path);
        let key = CredentialKey {
            provider: "goat-test-noexist".into(),
            account: "x".into(),
        };
        let cred = store.resolve(&key, Some("PATH")).unwrap();
        assert!(matches!(cred, ResolvedCredential::ApiKey(_)));
    }

    #[test]
    fn resolve_absent_is_none() {
        let path = std::env::temp_dir().join("goat-auth-absent-test.json");
        let _ = std::fs::remove_file(&path);
        let store = CredentialStore::new(path);
        let key = CredentialKey {
            provider: "goat-test-absent-xyz".into(),
            account: "none".into(),
        };
        assert!(
            store
                .resolve(&key, Some("GOAT_DEFINITELY_NOT_SET_VAR_42"))
                .is_none()
        );
    }
}
