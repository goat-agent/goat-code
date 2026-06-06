use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use goat_auth::{
    CredentialKey, CredentialStore, OAuthTokenSet, Pkce, ResolvedCredential, SecretString,
    capture_loopback_code, random_state,
};
use goat_provider::{
    AuthMethod, ModelEvent, ModelInfo, ModelProvider, ModelRequest, ProviderCapabilities,
    ProviderId,
};
use serde::Deserialize;
use tokio::{sync::mpsc, task::JoinHandle};

pub const PROVIDER_ID: &str = "openai-codex";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTHORIZE: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN: &str = "https://auth.openai.com/oauth/token";
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const CALLBACK_PORT: u16 = 1455;
const SCOPES: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
const ORIGINATOR: &str = "codex_cli_rs";
const BASE: &str = "https://chatgpt.com/backend-api/codex";
const DEFAULT_INSTRUCTIONS: &str = "You are goat, a coding assistant running in a terminal.";
const DEVICE_USERCODE: &str = "https://auth.openai.com/deviceauth/usercode";

const CATALOG: &[&str] = &["gpt-5.5", "gpt-5.4", "gpt-5.4-mini"];
const DEVICE_TOKEN: &str = "https://auth.openai.com/deviceauth/token";
const DEVICE_VERIFY_URL: &str = "https://auth.openai.com/codex/device";

const B64URL: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::URL_SAFE_NO_PAD;

#[derive(Debug, thiserror::Error)]
pub enum CodexError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("auth error: {0}")]
    Auth(#[from] goat_auth::AuthError),
    #[error("url error: {0}")]
    Url(String),
    #[error("token error: {0}")]
    Token(String),
    #[error("no browser available")]
    NoBrowser,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or(0)
}

fn authorize_url(challenge: &str, state: &str) -> Result<String, CodexError> {
    reqwest::Url::parse_with_params(
        AUTHORIZE,
        &[
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", REDIRECT_URI),
            ("scope", SCOPES),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("originator", ORIGINATOR),
            ("state", state),
        ],
    )
    .map(|url| url.to_string())
    .map_err(|err| CodexError::Url(err.to_string()))
}

fn account_id(access_token: &str) -> Option<String> {
    let payload = access_token.split('.').nth(1)?;
    let bytes = B64URL.decode(payload.trim_end_matches('=')).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims
        .get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .map(str::to_owned)
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

async fn exchange_code(code: &str, verifier: &str) -> Result<OAuthTokenSet, CodexError> {
    let response = reqwest::Client::new()
        .post(TOKEN)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", CLIENT_ID),
            ("code_verifier", verifier),
        ])
        .send()
        .await?;
    if let Err(err) = response.error_for_status_ref() {
        return Err(CodexError::Token(err.to_string()));
    }
    let tokens: TokenResponse = response.json().await?;
    let expires_at = tokens.expires_in.map(|seconds| now() + seconds);
    Ok(OAuthTokenSet {
        access_token: SecretString::from(tokens.access_token),
        refresh_token: tokens.refresh_token.map(SecretString::from),
        expires_at,
    })
}

fn browser_available() -> bool {
    if cfg!(any(target_os = "macos", target_os = "windows")) {
        return true;
    }
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

pub async fn login(status: &mpsc::Sender<String>) -> Result<OAuthTokenSet, CodexError> {
    if browser_available() {
        match login_browser().await {
            Err(CodexError::NoBrowser) => login_device(status).await,
            other => other,
        }
    } else {
        login_device(status).await
    }
}

async fn login_browser() -> Result<OAuthTokenSet, CodexError> {
    let pkce = Pkce::generate();
    let state = random_state();
    let url = authorize_url(&pkce.challenge, &state)?;
    if open::that(&url).is_err() {
        return Err(CodexError::NoBrowser);
    }
    let code = capture_loopback_code(CALLBACK_PORT, &state).await?;
    exchange_code(&code, &pkce.verifier).await
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_auth_id: String,
    user_code: String,
    interval: Option<u64>,
}

#[derive(Deserialize)]
struct DevicePollResponse {
    authorization_code: Option<String>,
    code_verifier: Option<String>,
}

async fn login_device(status: &mpsc::Sender<String>) -> Result<OAuthTokenSet, CodexError> {
    let client = reqwest::Client::new();
    let response = client
        .post(DEVICE_USERCODE)
        .json(&serde_json::json!({ "client_id": CLIENT_ID }))
        .send()
        .await?;
    if let Err(err) = response.error_for_status_ref() {
        return Err(CodexError::Token(err.to_string()));
    }
    let device: DeviceCodeResponse = response.json().await?;
    let _ = open::that(DEVICE_VERIFY_URL);
    let _ = status
        .send(format!(
            "open {DEVICE_VERIFY_URL} and enter code: {}",
            device.user_code
        ))
        .await;

    let interval = device.interval.unwrap_or(5).max(1);
    let deadline = now() + 900;
    loop {
        if now() > deadline {
            return Err(CodexError::Token("device login timed out".to_owned()));
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
        let poll = client
            .post(DEVICE_TOKEN)
            .json(&serde_json::json!({
                "device_auth_id": device.device_auth_id,
                "user_code": device.user_code,
            }))
            .send()
            .await?;
        if !poll.status().is_success() {
            continue;
        }
        let Ok(body) = poll.json::<DevicePollResponse>().await else {
            continue;
        };
        if let (Some(code), Some(verifier)) = (body.authorization_code, body.code_verifier) {
            return exchange_code(&code, &verifier).await;
        }
    }
}

pub async fn refresh_tokens(refresh_token: &str) -> Result<OAuthTokenSet, CodexError> {
    let response = reqwest::Client::new()
        .post(TOKEN)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await?;
    if let Err(err) = response.error_for_status_ref() {
        return Err(CodexError::Token(err.to_string()));
    }
    let tokens: TokenResponse = response.json().await?;
    let expires_at = tokens.expires_in.map(|seconds| now() + seconds);
    Ok(OAuthTokenSet {
        access_token: SecretString::from(tokens.access_token),
        refresh_token: tokens
            .refresh_token
            .map(SecretString::from)
            .or_else(|| Some(SecretString::from(refresh_token))),
        expires_at,
    })
}

fn is_expired(tokens: &OAuthTokenSet) -> bool {
    tokens.expires_at.is_some_and(|exp| exp <= now() + 60)
}

async fn current_access(
    store: &CredentialStore,
    key: &CredentialKey,
) -> Option<(String, Option<String>)> {
    let tokens = match store.resolve(key, None)? {
        ResolvedCredential::OAuth(tokens) => tokens,
        ResolvedCredential::ApiKey(secret) => {
            let access = secret.expose().to_owned();
            let account = account_id(&access);
            return Some((access, account));
        }
    };
    let tokens = if is_expired(&tokens) {
        match tokens.refresh_token.as_ref() {
            Some(refresh) => match refresh_tokens(refresh.expose()).await {
                Ok(fresh) => {
                    let _ = store.store(key, ResolvedCredential::OAuth(fresh.clone()));
                    fresh
                }
                Err(_) => tokens,
            },
            None => tokens,
        }
    } else {
        tokens
    };
    let access = tokens.access_token.expose().to_owned();
    let account = account_id(&access);
    Some((access, account))
}

pub fn build(store: &CredentialStore, account: &str) -> CodexProvider {
    let key = CredentialKey {
        provider: PROVIDER_ID.to_owned(),
        account: account.to_owned(),
    };
    CodexProvider::new(store.clone(), key)
}

pub struct CodexProvider {
    store: CredentialStore,
    key: CredentialKey,
    client: reqwest::Client,
}

impl CodexProvider {
    pub fn new(store: CredentialStore, key: CredentialKey) -> Self {
        Self {
            store,
            key,
            client: reqwest::Client::new(),
        }
    }
}

async fn fetch_models(
    client: &reqwest::Client,
    access: &str,
    account: Option<&str>,
) -> Vec<ModelInfo> {
    let mut builder = client
        .get(format!("{BASE}/models?client_version=0.0.0"))
        .bearer_auth(access)
        .header("Accept", "application/json");
    if let Some(account) = account {
        builder = builder.header("chatgpt-account-id", account);
    }
    let Ok(response) = builder.send().await else {
        return Vec::new();
    };
    if !response.status().is_success() {
        return Vec::new();
    }
    let Ok(value) = response.json::<serde_json::Value>().await else {
        return Vec::new();
    };
    let Some(items) = value.get("models").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter(|model| model.get("visibility").and_then(serde_json::Value::as_str) == Some("list"))
        .filter_map(|model| {
            let id = model.get("slug").and_then(serde_json::Value::as_str)?;
            Some(ModelInfo { id: id.to_owned() })
        })
        .collect()
}

impl ModelProvider for CodexProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from(PROVIDER_ID)
    }

    fn authenticated(&self) -> bool {
        self.store.resolve(&self.key, None).is_some()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            tools: false,
            auth: AuthMethod::OAuth,
        }
    }

    fn request(&self, req: ModelRequest, events: mpsc::Sender<ModelEvent>) -> JoinHandle<()> {
        let client = self.client.clone();
        let url = format!("{BASE}/responses");
        let store = self.store.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let Some((access, account)) = current_access(&store, &key).await else {
                let _ = events
                    .send(ModelEvent::Failed {
                        message: "not logged in to codex".to_owned(),
                    })
                    .await;
                return;
            };
            let body = goat_provider_responses::build_body(
                &req.model,
                &req.messages,
                Some(DEFAULT_INSTRUCTIONS),
                false,
            );
            goat_provider_responses::run_request(
                &client,
                &url,
                Some(&access),
                account.as_deref(),
                &body,
                &events,
            )
            .await;
        })
    }

    fn catalog(&self) -> &'static [&'static str] {
        CATALOG
    }

    fn discover(&self, out: mpsc::Sender<ModelInfo>) -> JoinHandle<()> {
        let client = self.client.clone();
        let store = self.store.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let Some((access, account)) = current_access(&store, &key).await else {
                return;
            };
            for model in fetch_models(&client, &access, account.as_deref()).await {
                if out.send(model).await.is_err() {
                    return;
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{account_id, authorize_url};

    #[test]
    fn authorize_url_carries_pkce_and_client() {
        let url = authorize_url("CHAL", "STATE").unwrap();
        assert!(url.contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann"));
        assert!(url.contains("code_challenge=CHAL"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=STATE"));
        assert!(url.contains("redirect_uri=http"));
        assert!(url.contains("codex_cli_simplified_flow=true"));
        assert!(url.contains("id_token_add_organizations=true"));
    }

    #[test]
    fn decodes_account_id_from_jwt() {
        use base64::Engine;
        let payload = r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct-99"}}"#;
        let encoded = super::B64URL.encode(payload.as_bytes());
        let jwt = format!("header.{encoded}.sig");
        assert_eq!(account_id(&jwt).as_deref(), Some("acct-99"));
    }
}
