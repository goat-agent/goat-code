use std::path::PathBuf;

use goat_auth::{Credential, CredentialKey, CredentialStore, TokenSet, ensure_valid};
use goat_provider::{
    AuthMethod, Capabilities, Effort, Model, Provider, ProviderId, ProviderMetadata, Request,
    StreamError, StreamEvent, WebSearchOutput,
};
use goat_provider_openai_compat::{ChatDiscovery, ChatValidation, OpenAiCompatProvider};
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use serde::Deserialize;
use tokio::{sync::mpsc, task::JoinHandle};

pub const OPENROUTER: &str = "openrouter";
pub const GROQ: &str = "groq";
pub const DEEPSEEK: &str = "deepseek";
pub const XAI: &str = "xai";
pub const MISTRAL: &str = "mistral";
pub const ZAI: &str = "zai";
pub const ZAI_CODING: &str = "zai-coding";
pub const KIMI: &str = "kimi";
pub const KIMI_CODE: &str = "kimi-code";
pub const QWEN: &str = "qwen";

const KIMI_CODE_BASE_URL: &str = "https://api.kimi.com/coding/v1";
const KIMI_CODE_OAUTH_HOST: &str = "https://auth.kimi.com";
const KIMI_CODE_CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";

const OPENROUTER_CATALOG: &[&str] = &[
    "anthropic/claude-sonnet-4.5",
    "openai/gpt-5.1",
    "google/gemini-2.5-pro",
    "deepseek/deepseek-chat-v3.1",
    "qwen/qwen3-coder",
    "moonshotai/kimi-k2",
];

const GROQ_CATALOG: &[&str] = &[
    "openai/gpt-oss-120b",
    "openai/gpt-oss-20b",
    "moonshotai/kimi-k2-instruct-0905",
    "qwen/qwen3-32b",
    "deepseek-r1-distill-llama-70b",
    "llama-3.3-70b-versatile",
];

const DEEPSEEK_CATALOG: &[&str] = &["deepseek-chat", "deepseek-reasoner"];

const XAI_CATALOG: &[&str] = &[
    "grok-4",
    "grok-4-fast-reasoning",
    "grok-4-fast-non-reasoning",
    "grok-3",
    "grok-3-fast",
    "grok-3-mini",
];

const MISTRAL_CATALOG: &[&str] = &[
    "mistral-large-latest",
    "mistral-medium-latest",
    "mistral-small-latest",
    "devstral-medium-latest",
    "codestral-latest",
    "ministral-8b-latest",
];

const ZAI_CATALOG: &[&str] = &[
    "glm-5.2",
    "glm-5.1",
    "glm-5-turbo",
    "glm-5",
    "glm-4.7",
    "glm-4.6",
    "glm-4.5",
    "glm-4-32b-0414-128k",
];

const ZAI_CODING_CATALOG: &[&str] = &["glm-5.2", "glm-5-turbo", "glm-4.7"];

const KIMI_CATALOG: &[&str] = &[
    "kimi-k2.7-code",
    "kimi-k2.7-code-highspeed",
    "kimi-k2.6",
    "kimi-k2.5",
    "moonshot-v1-128k",
    "moonshot-v1-32k",
    "moonshot-v1-8k",
];

const KIMI_CODE_CATALOG: &[&str] = &[
    "kimi-k2.7-code",
    "kimi-k2.7-code-highspeed",
    "kimi-k2.6",
    "kimi-k2.5",
];

const QWEN_CATALOG: &[&str] = &[
    "qwen-plus",
    "qwen-max",
    "qwen-turbo",
    "qwen3-coder-plus",
    "qwen3-coder-flash",
    "qwen-vl-plus",
];

const OPENROUTER_CONTEXT: &[(&str, u32)] = &[
    ("anthropic/claude-sonnet-4.5", 200_000),
    ("openai/gpt-5", 400_000),
    ("google/gemini-2.5", 1_000_000),
    ("deepseek/deepseek", 128_000),
    ("qwen/qwen3-coder", 256_000),
    ("moonshotai/kimi", 256_000),
];

const GROQ_CONTEXT: &[(&str, u32)] = &[
    ("openai/gpt-oss-120b", 131_072),
    ("openai/gpt-oss-20b", 131_072),
    ("moonshotai/kimi", 131_072),
    ("qwen/qwen3", 131_072),
    ("llama-3.3", 131_072),
];

const DEEPSEEK_CONTEXT: &[(&str, u32)] =
    &[("deepseek-chat", 128_000), ("deepseek-reasoner", 128_000)];

const XAI_CONTEXT: &[(&str, u32)] = &[("grok-4", 256_000), ("grok-3", 131_072)];

const MISTRAL_CONTEXT: &[(&str, u32)] = &[
    ("mistral-large", 131_072),
    ("mistral-medium", 131_072),
    ("mistral-small", 131_072),
    ("devstral-medium", 131_072),
    ("codestral", 256_000),
];

const ZAI_CONTEXT: &[(&str, u32)] = &[
    ("glm-5.2", 128_000),
    ("glm-5.1", 128_000),
    ("glm-5", 128_000),
    ("glm-4", 128_000),
];

const ZAI_CODING_CONTEXT: &[(&str, u32)] = &[
    ("glm-5.2", 1_000_000),
    ("glm-5-turbo", 128_000),
    ("glm-4.7", 128_000),
];

const KIMI_CONTEXT: &[(&str, u32)] = &[
    ("kimi-k2.7", 256_000),
    ("kimi-k2.6", 256_000),
    ("kimi-k2.5", 256_000),
    ("moonshot-v1-128k", 128_000),
    ("moonshot-v1-32k", 32_000),
    ("moonshot-v1-8k", 8_000),
];

const KIMI_CODE_CONTEXT: &[(&str, u32)] = &[
    ("kimi-k2.7", 256_000),
    ("kimi-k2.6", 256_000),
    ("kimi-k2.5", 256_000),
];

const QWEN_CONTEXT: &[(&str, u32)] = &[
    ("qwen-plus", 131_072),
    ("qwen-max", 131_072),
    ("qwen-turbo", 1_000_000),
    ("qwen3-coder", 1_000_000),
    ("qwen-vl", 129_024),
];

const HOSTS: &[(&str, &str)] = &[
    (OPENROUTER, "openrouter.ai"),
    (GROQ, "api.groq.com"),
    (DEEPSEEK, "api.deepseek.com"),
    (XAI, "api.x.ai"),
    (MISTRAL, "api.mistral.ai"),
    (ZAI, "api.z.ai"),
    (ZAI_CODING, "api.z.ai"),
    (KIMI, "api.moonshot.ai"),
    (KIMI_CODE, "api.kimi.com"),
    (QWEN, "dashscope-us.aliyuncs.com"),
];

#[derive(Debug, thiserror::Error)]
pub enum HostedOAuthError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("oauth error: {0}")]
    OAuth(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub fn all(store: &CredentialStore, account: &str) -> Vec<OpenAiCompatProvider> {
    vec![
        build_openrouter(store, account),
        build_groq(store, account),
        build_deepseek(store, account),
        build_xai(store, account),
        build_mistral(store, account),
        build_zai(store, account),
        build_zai_coding(store, account),
        build_kimi(store, account),
        build_qwen(store, account),
    ]
}

pub fn build_openrouter(store: &CredentialStore, account: &str) -> OpenAiCompatProvider {
    hosted(
        OPENROUTER,
        "https://openrouter.ai/api/v1",
        "OPENROUTER_API_KEY",
        store,
        account,
    )
    .with_catalog(OPENROUTER_CATALOG)
    .with_context_windows(OPENROUTER_CONTEXT)
    .with_model_filter(openrouter_chat_model)
    .with_vision_filter(openrouter_vision_model)
    .with_reasoning_effort(false)
}

pub fn build_groq(store: &CredentialStore, account: &str) -> OpenAiCompatProvider {
    hosted(
        GROQ,
        "https://api.groq.com/openai/v1",
        "GROQ_API_KEY",
        store,
        account,
    )
    .with_catalog(GROQ_CATALOG)
    .with_context_windows(GROQ_CONTEXT)
    .with_model_filter(groq_chat_model)
    .with_images(false)
    .with_stream_options(false)
    .with_reasoning_effort(false)
}

pub fn build_deepseek(store: &CredentialStore, account: &str) -> OpenAiCompatProvider {
    hosted(
        DEEPSEEK,
        "https://api.deepseek.com",
        "DEEPSEEK_API_KEY",
        store,
        account,
    )
    .with_catalog(DEEPSEEK_CATALOG)
    .with_context_windows(DEEPSEEK_CONTEXT)
    .with_images(false)
    .with_reasoning_effort(false)
}

pub fn build_xai(store: &CredentialStore, account: &str) -> OpenAiCompatProvider {
    hosted(XAI, "https://api.x.ai/v1", "XAI_API_KEY", store, account)
        .with_catalog(XAI_CATALOG)
        .with_context_windows(XAI_CONTEXT)
        .with_vision_filter(xai_vision_model)
        .with_efforts(no_efforts)
        .with_reasoning_effort(false)
}

pub fn build_mistral(store: &CredentialStore, account: &str) -> OpenAiCompatProvider {
    hosted(
        MISTRAL,
        "https://api.mistral.ai/v1",
        "MISTRAL_API_KEY",
        store,
        account,
    )
    .with_catalog(MISTRAL_CATALOG)
    .with_context_windows(MISTRAL_CONTEXT)
    .with_vision_filter(mistral_vision_model)
    .with_efforts(no_efforts)
    .with_reasoning_effort(false)
}

pub fn build_zai(store: &CredentialStore, account: &str) -> OpenAiCompatProvider {
    hosted(
        ZAI,
        "https://api.z.ai/api/paas/v4",
        "ZAI_API_KEY",
        store,
        account,
    )
    .with_catalog(ZAI_CATALOG)
    .with_context_windows(ZAI_CONTEXT)
    .with_vision_filter(zai_vision_model)
    .with_efforts(zai_efforts)
    .with_effort_wire(zai_effort_wire)
    .with_validation(ChatValidation::CatalogOnly)
    .with_discovery(ChatDiscovery::CatalogOnly)
    .with_metadata(ProviderMetadata {
        env_var: Some("ZAI_API_KEY"),
        validation: "catalog-only",
        endpoint: None,
        oauth: Some("not supported by Z.AI API docs"),
    })
}

pub fn build_zai_coding(store: &CredentialStore, account: &str) -> OpenAiCompatProvider {
    hosted(
        ZAI_CODING,
        "https://api.z.ai/api/coding/paas/v4",
        "ZAI_CODING_API_KEY",
        store,
        account,
    )
    .with_catalog(ZAI_CODING_CATALOG)
    .with_context_windows(ZAI_CODING_CONTEXT)
    .with_vision_filter(no_vision)
    .with_efforts(zai_efforts)
    .with_effort_wire(zai_effort_wire)
    .with_validation(ChatValidation::CatalogOnly)
    .with_discovery(ChatDiscovery::CatalogOnly)
    .with_metadata(ProviderMetadata {
        env_var: Some("ZAI_CODING_API_KEY"),
        validation: "catalog-only",
        endpoint: Some("https://api.z.ai/api/coding/paas/v4"),
        oauth: Some("not OAuth; uses Z.AI Coding Plan API key"),
    })
}

pub fn build_kimi(store: &CredentialStore, account: &str) -> OpenAiCompatProvider {
    hosted(
        KIMI,
        "https://api.moonshot.ai/v1",
        "MOONSHOT_API_KEY",
        store,
        account,
    )
    .with_catalog(KIMI_CATALOG)
    .with_context_windows(KIMI_CONTEXT)
    .with_vision_filter(no_vision)
    .with_efforts(no_efforts)
    .with_reasoning_effort(false)
    .with_validation(ChatValidation::CatalogOnly)
    .with_discovery(ChatDiscovery::CatalogOnly)
    .with_metadata(ProviderMetadata {
        env_var: Some("MOONSHOT_API_KEY"),
        validation: "catalog-only",
        endpoint: None,
        oauth: Some("Kimi Code OAuth is provider id kimi-code"),
    })
}

pub fn build_kimi_code(store: &CredentialStore, account: &str) -> KimiCodeProvider {
    enforce_host(KIMI_CODE, KIMI_CODE_BASE_URL).expect("kimi-code provider base URL");
    KimiCodeProvider::new(store.clone(), CredentialKey::model(KIMI_CODE, account))
}

pub fn build_qwen(store: &CredentialStore, account: &str) -> OpenAiCompatProvider {
    let key = CredentialKey::model(QWEN, account);
    let stored = store.get(&key);
    let endpoint_source = std::env::var("QWEN_BASE_URL").ok().or_else(|| {
        stored
            .as_ref()
            .and_then(goat_auth::Credential::endpoint)
            .map(str::to_owned)
    });
    let endpoint = match endpoint_source {
        Some(raw) => validate_qwen_endpoint(&raw).ok(),
        None => Some("https://dashscope-us.aliyuncs.com/compatible-mode/v1".to_owned()),
    };
    let bearer = endpoint.as_ref().and_then(|_| {
        store
            .resolve(&key, Some("DASHSCOPE_API_KEY"))
            .map(|cred| cred.bearer().to_owned())
    });
    OpenAiCompatProvider::new(
        ProviderId::from(QWEN),
        endpoint
            .unwrap_or_else(|| "https://dashscope-us.aliyuncs.com/compatible-mode/v1".to_owned()),
        bearer,
        AuthMethod::ApiKey,
    )
    .with_catalog(QWEN_CATALOG)
    .with_context_windows(QWEN_CONTEXT)
    .with_vision_filter(qwen_vision_model)
    .with_efforts(no_efforts)
    .with_reasoning_effort(false)
    .with_metadata(ProviderMetadata {
        env_var: Some("DASHSCOPE_API_KEY"),
        validation: "network",
        endpoint: Some("required for non-US DashScope workspaces"),
        oauth: Some("Qwen OAuth enrollment discontinued"),
    })
}

fn hosted(
    provider_id: &'static str,
    base_url: &'static str,
    env_var: &'static str,
    store: &CredentialStore,
    account: &str,
) -> OpenAiCompatProvider {
    enforce_host(provider_id, base_url).expect("hosted provider base URL");
    let key = CredentialKey::model(provider_id, account);
    let bearer = store
        .resolve(&key, Some(env_var))
        .map(|cred| cred.bearer().to_owned());
    OpenAiCompatProvider::new(
        ProviderId::from(provider_id),
        base_url,
        bearer,
        AuthMethod::ApiKey,
    )
    .with_metadata(ProviderMetadata {
        env_var: Some(env_var),
        validation: "network",
        endpoint: None,
        oauth: Some("not supported"),
    })
}

fn enforce_host(provider_id: &str, base_url: &str) -> Result<(), String> {
    let Some(host) = HOSTS
        .iter()
        .find_map(|(id, host)| (*id == provider_id).then_some(*host))
    else {
        return Err(format!("unknown hosted provider: {provider_id}"));
    };
    let url = base_url.trim_end_matches('/');
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| "hosted providers require https".to_owned())?;
    let actual = rest.split('/').next().unwrap_or_default();
    if actual == host || actual.ends_with(&format!(".{host}")) {
        Ok(())
    } else {
        Err(format!("invalid hosted provider host: {actual}"))
    }
}

pub struct KimiCodeProvider {
    store: CredentialStore,
    key: CredentialKey,
    client: reqwest::Client,
}

impl KimiCodeProvider {
    pub fn new(store: CredentialStore, key: CredentialKey) -> Self {
        Self {
            store,
            key,
            client: oauth_client(),
        }
    }
}

impl Provider for KimiCodeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from(KIMI_CODE)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tools: true,
            auth: AuthMethod::OAuth,
            images: false,
        }
    }

    fn metadata(&self) -> ProviderMetadata {
        kimi_code_metadata()
    }

    fn authenticated(&self) -> bool {
        self.store
            .get(&self.key)
            .is_some_and(|cred| matches!(cred, Credential::OAuth(_)))
    }

    fn catalog(&self) -> &'static [&'static str] {
        KIMI_CODE_CATALOG
    }

    fn efforts(&self, _model: &str) -> Vec<Effort> {
        Vec::new()
    }

    fn context_window(&self, model: &str) -> Option<u32> {
        KIMI_CODE_CONTEXT
            .iter()
            .find_map(|(prefix, window)| model.starts_with(prefix).then_some(*window))
    }

    fn supports_images(&self, _model: &str) -> bool {
        false
    }

    fn verifies_credentials(&self) -> bool {
        true
    }

    fn validate(&self) -> JoinHandle<Result<(), String>> {
        let store = self.store.clone();
        let key = self.key.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            let Some(token) = current_kimi_code_token(&store, &key).await else {
                return Err("no credentials".to_owned());
            };
            let response = client
                .get(format!("{KIMI_CODE_BASE_URL}/models"))
                .bearer_auth(token)
                .send()
                .await
                .map_err(|_| "could not reach provider".to_owned())?;
            let status = response.status();
            if status.is_success() {
                Ok(())
            } else if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                Err("invalid credentials".to_owned())
            } else {
                Err(format!("could not reach provider: {status}"))
            }
        })
    }

    fn stream(&self, req: Request, events: mpsc::Sender<StreamEvent>) -> JoinHandle<()> {
        let store = self.store.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let Some(token) = current_kimi_code_token(&store, &key).await else {
                let _ = events
                    .send(StreamEvent::Failed {
                        error: StreamError::auth("no credentials"),
                    })
                    .await;
                return;
            };
            let provider = OpenAiCompatProvider::new(
                ProviderId::from(KIMI_CODE),
                KIMI_CODE_BASE_URL,
                Some(token),
                AuthMethod::OAuth,
            )
            .with_catalog(KIMI_CODE_CATALOG)
            .with_context_windows(KIMI_CODE_CONTEXT)
            .with_vision_filter(no_vision)
            .with_efforts(no_efforts)
            .with_reasoning_effort(false);
            let handle = provider.stream(req, events);
            let _ = handle.await;
        })
    }

    fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
        let store = self.store.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let Some(token) = current_kimi_code_token(&store, &key).await else {
                for id in KIMI_CODE_CATALOG {
                    if out
                        .send(Model {
                            id: (*id).to_owned(),
                            supports_images: false,
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                return;
            };
            let provider = OpenAiCompatProvider::new(
                ProviderId::from(KIMI_CODE),
                KIMI_CODE_BASE_URL,
                Some(token),
                AuthMethod::OAuth,
            )
            .with_model_filter(kimi_code_chat_model)
            .with_vision_filter(no_vision);
            let handle = provider.discover(out);
            let _ = handle.await;
        })
    }

    fn login(&self, status: mpsc::Sender<String>) -> JoinHandle<Result<TokenSet, String>> {
        tokio::spawn(async move {
            kimi_code_login(&status)
                .await
                .map_err(|err| err.to_string())
        })
    }

    fn web_search(&self, query: String) -> JoinHandle<Result<WebSearchOutput, StreamError>> {
        let _ = query;
        tokio::spawn(async { Err(StreamError::other("web search is not supported")) })
    }
}

fn kimi_code_metadata() -> ProviderMetadata {
    ProviderMetadata {
        env_var: None,
        validation: "network",
        endpoint: Some(KIMI_CODE_BASE_URL),
        oauth: Some("device code"),
    }
}

async fn current_kimi_code_token(store: &CredentialStore, key: &CredentialKey) -> Option<String> {
    let Credential::OAuth(tokens) = store.get(key)? else {
        return None;
    };
    let tokens = ensure_valid(tokens, store, key, kimi_code_refresh).await?;
    Some(tokens.access_token.expose().to_owned())
}

fn oauth_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("reqwest client")
}

#[derive(Deserialize)]
struct DeviceAuthorizationResponse {
    user_code: String,
    device_code: String,
    verification_uri: Option<String>,
    verification_uri_complete: String,
    expires_in: Option<u64>,
    interval: Option<u64>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
    scope: Option<String>,
    token_type: Option<String>,
}

#[derive(Deserialize)]
struct OAuthErrorResponse {
    error: Option<String>,
    #[serde(rename = "error_description")]
    _error_description: Option<String>,
}

async fn kimi_code_login(status: &mpsc::Sender<String>) -> Result<TokenSet, HostedOAuthError> {
    let client = oauth_client();
    let device = request_device_authorization(&client).await?;
    let url = if device.verification_uri_complete.is_empty() {
        device.verification_uri.as_deref().unwrap_or("")
    } else {
        &device.verification_uri_complete
    };
    if !valid_kimi_verification_url(url) {
        return Err(HostedOAuthError::OAuth(
            "device authorization returned an invalid verification URL".to_owned(),
        ));
    }
    let _ = status
        .send(format!("open {url} and enter code: {}", device.user_code))
        .await;
    poll_device_token(&client, &device).await
}

async fn request_device_authorization(
    client: &reqwest::Client,
) -> Result<DeviceAuthorizationResponse, HostedOAuthError> {
    let response = client
        .post(format!(
            "{KIMI_CODE_OAUTH_HOST}/api/oauth/device_authorization"
        ))
        .headers(kimi_headers())
        .form(&[("client_id", KIMI_CODE_CLIENT_ID)])
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(HostedOAuthError::OAuth(format!(
            "device authorization failed: {status}"
        )));
    }
    let device: DeviceAuthorizationResponse = response.json().await?;
    if device.user_code.is_empty()
        || device.device_code.is_empty()
        || device.verification_uri_complete.is_empty()
    {
        return Err(HostedOAuthError::OAuth(
            "device authorization response is missing required fields".to_owned(),
        ));
    }
    Ok(device)
}

async fn poll_device_token(
    client: &reqwest::Client,
    device: &DeviceAuthorizationResponse,
) -> Result<TokenSet, HostedOAuthError> {
    let mut interval = device.interval.unwrap_or(5).max(1);
    let deadline =
        goat_auth::now_secs() + i64::try_from(device.expires_in.unwrap_or(900)).unwrap_or(900);
    loop {
        if goat_auth::now_secs() > deadline {
            return Err(HostedOAuthError::OAuth("device login timed out".to_owned()));
        }
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
        let response = client
            .post(format!("{KIMI_CODE_OAUTH_HOST}/api/oauth/token"))
            .headers(kimi_headers())
            .form(&[
                ("client_id", KIMI_CODE_CLIENT_ID),
                ("device_code", device.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await?;
        let status = response.status();
        if status.is_success() {
            let tokens: TokenResponse = response.json().await?;
            return parse_token_response(tokens);
        }
        let error = response.json::<OAuthErrorResponse>().await.ok();
        match error.as_ref().and_then(|err| err.error.as_deref()) {
            Some("authorization_pending") => {}
            Some("slow_down") => interval = interval.saturating_add(5).min(30),
            Some("expired_token") => {
                return Err(HostedOAuthError::OAuth(
                    "device login code expired".to_owned(),
                ));
            }
            Some("access_denied") => {
                return Err(HostedOAuthError::OAuth(
                    "device login access denied".to_owned(),
                ));
            }
            Some(code) => {
                return Err(HostedOAuthError::OAuth(format!(
                    "device token polling failed: {code}"
                )));
            }
            None => {
                return Err(HostedOAuthError::OAuth(format!(
                    "device token polling failed: {status}"
                )));
            }
        }
    }
}

async fn kimi_code_refresh(refresh_token: String) -> Result<TokenSet, String> {
    let client = oauth_client();
    let response = client
        .post(format!("{KIMI_CODE_OAUTH_HOST}/api/oauth/token"))
        .headers(kimi_headers())
        .form(&[
            ("client_id", KIMI_CODE_CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
        ])
        .send()
        .await
        .map_err(|_| "token refresh request failed".to_owned())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("token refresh failed: {status}"));
    }
    response
        .json::<TokenResponse>()
        .await
        .map_err(|_| "token refresh returned invalid JSON".to_owned())
        .and_then(|tokens| parse_token_response(tokens).map_err(|err| err.to_string()))
}

fn parse_token_response(tokens: TokenResponse) -> Result<TokenSet, HostedOAuthError> {
    let _ = (&tokens.scope, &tokens.token_type);
    if tokens.access_token.is_empty() || tokens.refresh_token.is_empty() || tokens.expires_in <= 0 {
        return Err(HostedOAuthError::OAuth(
            "token response is missing required fields".to_owned(),
        ));
    }
    Ok(TokenSet::from_parts(
        tokens.access_token,
        Some(tokens.refresh_token),
        Some(tokens.expires_in),
        None,
    ))
}

fn kimi_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    headers.insert(USER_AGENT, HeaderValue::from_static("goat-code/0.1.14"));
    insert_header(&mut headers, "X-Msh-Platform", "kimi_code_cli");
    insert_header(&mut headers, "X-Msh-Version", env!("CARGO_PKG_VERSION"));
    insert_header(&mut headers, "X-Msh-Device-Name", &device_name());
    insert_header(&mut headers, "X-Msh-Device-Model", &device_model());
    insert_header(&mut headers, "X-Msh-Os-Version", std::env::consts::OS);
    insert_header(&mut headers, "X-Msh-Device-Id", &device_id());
    headers
}

fn insert_header(headers: &mut HeaderMap, name: &'static str, value: &str) {
    if let Ok(value) = HeaderValue::from_str(&ascii_header(value)) {
        headers.insert(
            HeaderName::from_static(name.to_ascii_lowercase().leak()),
            value,
        );
    }
}

fn ascii_header(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|ch| matches!(*ch as u32, 0x20..=0x7e))
        .collect::<String>()
        .trim()
        .to_owned();
    if cleaned.is_empty() {
        "unknown".to_owned()
    } else {
        cleaned
    }
}

fn device_name() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_owned())
}

fn device_model() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

fn device_id() -> String {
    let path = device_id_path();
    if let Ok(value) = std::fs::read_to_string(&path) {
        let value = value.trim();
        if !value.is_empty() {
            return value.to_owned();
        }
    }
    let id = goat_auth::random_state();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
        set_private_dir(parent);
    }
    if std::fs::write(&path, &id).is_ok() {
        set_private_file(&path);
    }
    id
}

fn device_id_path() -> PathBuf {
    std::env::home_dir().map_or_else(
        || PathBuf::from(".goat-code-kimi-code-device-id"),
        |home| home.join(".goat-code").join("kimi-code-device-id"),
    )
}

#[cfg(unix)]
fn set_private_dir(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn set_private_dir(_path: &std::path::Path) {}

#[cfg(unix)]
fn set_private_file(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_private_file(_path: &std::path::Path) {}

fn valid_kimi_verification_url(url: &str) -> bool {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|url| {
            (url.scheme() == "https")
                .then(|| url.host_str().is_some_and(|host| host == "auth.kimi.com"))
        })
        .unwrap_or(false)
}

fn openrouter_chat_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    !(id.contains("embedding")
        || id.contains("moderation")
        || id.contains("image")
        || id.contains("tts")
        || id.contains("whisper"))
}

fn groq_chat_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    !(id.contains("whisper") || id.contains("tts") || id.contains("embedding"))
}

fn kimi_code_chat_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    !id.contains("embedding") && !id.contains("image") && !id.contains("video")
}

fn openrouter_vision_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    goat_provider_openai_compat::known_openai_compatible_vision_model(&id)
        || id.contains("claude")
        || id.contains("gemini")
        || id.contains("grok-4")
}

fn xai_vision_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id.starts_with("grok-4") || id.contains("vision")
}

fn mistral_vision_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id.contains("pixtral")
}

fn zai_vision_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id.contains("glm-4v") || id.contains("vision")
}

fn qwen_vision_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id.contains("qwen-vl") || id.contains("qwen2-vl") || id.contains("qwen2.5-vl")
}

pub fn validate_qwen_endpoint(endpoint: &str) -> Result<String, String> {
    let trimmed = endpoint.trim().trim_end_matches('/');
    let url = reqwest::Url::parse(trimmed).map_err(|err| err.to_string())?;
    if url.scheme() != "https" {
        return Err("qwen endpoint must use https".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("qwen endpoint must not include userinfo".to_owned());
    }
    let Some(host) = url.host_str() else {
        return Err("qwen endpoint must include a host".to_owned());
    };
    if host.ends_with('.') {
        return Err("qwen endpoint host must not end with a dot".to_owned());
    }
    let allowed_static = [
        "dashscope.aliyuncs.com",
        "dashscope-intl.aliyuncs.com",
        "dashscope-us.aliyuncs.com",
    ];
    let allowed_regions = [
        "cn-beijing.maas.aliyuncs.com",
        "ap-southeast-1.maas.aliyuncs.com",
        "ap-northeast-1.maas.aliyuncs.com",
    ];
    let allowed = allowed_static.contains(&host)
        || allowed_regions.iter().any(|region| {
            host.strip_suffix(region)
                .and_then(|prefix| prefix.strip_suffix('.'))
                .is_some_and(valid_workspace_id)
        });
    if !allowed {
        return Err("qwen endpoint host is not an allowed Alibaba Model Studio host".to_owned());
    }
    if url.port().is_some() {
        return Err("qwen endpoint must not include a custom port".to_owned());
    }
    if url.path() != "/compatible-mode/v1" {
        return Err("qwen endpoint path must be /compatible-mode/v1".to_owned());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("qwen endpoint must not include query or fragment".to_owned());
    }
    Ok(trimmed.to_owned())
}

fn valid_workspace_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn no_vision(_id: &str) -> bool {
    false
}

fn no_efforts(_model: &str) -> Vec<Effort> {
    Vec::new()
}

fn zai_efforts(model: &str) -> Vec<Effort> {
    if model == "glm-5.2" {
        vec![
            Effort::Off,
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::Xhigh,
            Effort::Max,
        ]
    } else {
        Vec::new()
    }
}

fn zai_effort_wire(effort: Effort) -> Option<&'static str> {
    let wire = match effort {
        Effort::Off => "none",
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Xhigh => "xhigh",
        Effort::Max => "max",
    };
    (!wire.is_empty()).then_some(wire)
}

#[cfg(test)]
mod tests {
    use goat_auth::{Credential, SecretString};
    use goat_provider::{AuthMethod, Effort, Provider};

    use super::*;

    fn store(name: &str) -> CredentialStore {
        let _ = std::fs::remove_file(std::env::temp_dir().join(name));
        CredentialStore::new(std::env::temp_dir().join(name))
    }

    #[test]
    fn enforces_https_and_provider_owned_hosts() {
        assert!(enforce_host(OPENROUTER, "https://openrouter.ai/api/v1/").is_ok());
        assert!(enforce_host(OPENROUTER, "http://openrouter.ai/api/v1").is_err());
        assert!(enforce_host(OPENROUTER, "https://example.com/api/v1").is_err());
        assert!(enforce_host(ZAI_CODING, "https://api.z.ai/api/coding/paas/v4").is_ok());
        assert!(enforce_host(KIMI_CODE, KIMI_CODE_BASE_URL).is_ok());
    }

    #[test]
    fn resolves_stored_credential() {
        let store = store("goat-provider-hosted-resolves.json");
        store
            .store(
                &CredentialKey::model(OPENROUTER, "default"),
                Credential::ApiKey(SecretString::from("key".to_owned())),
            )
            .unwrap();
        let provider = build_openrouter(&store, "default");
        assert!(provider.authenticated());
        assert_eq!(provider.capabilities().auth, AuthMethod::ApiKey);
    }

    #[test]
    fn metadata_is_exposed() {
        let store = store("goat-provider-hosted-metadata.json");
        let provider = build_zai(&store, "default");
        assert_eq!(provider.catalog(), ZAI_CATALOG);
        assert_eq!(provider.context_window("glm-5.2"), Some(128_000));
        assert_eq!(
            provider.efforts("glm-5.2"),
            vec![
                Effort::Off,
                Effort::Low,
                Effort::Medium,
                Effort::High,
                Effort::Xhigh,
                Effort::Max
            ]
        );
        assert!(!provider.verifies_credentials());
    }

    #[test]
    fn zai_coding_is_distinct_api_key_provider() {
        let store = store("goat-provider-hosted-zai-coding.json");
        let provider = build_zai_coding(&store, "default");
        assert_eq!(provider.capabilities().auth, AuthMethod::ApiKey);
        assert_eq!(provider.metadata().env_var, Some("ZAI_CODING_API_KEY"));
        assert_eq!(
            provider.metadata().endpoint,
            Some("https://api.z.ai/api/coding/paas/v4")
        );
        assert_eq!(provider.catalog(), ZAI_CODING_CATALOG);
        assert_eq!(provider.context_window("glm-5.2"), Some(1_000_000));
    }

    #[test]
    fn kimi_code_is_oauth_provider() {
        let store = store("goat-provider-hosted-kimi-code.json");
        let provider = build_kimi_code(&store, "default");
        assert_eq!(provider.capabilities().auth, AuthMethod::OAuth);
        assert_eq!(provider.metadata().oauth, Some("device code"));
        assert!(!provider.authenticated());
        assert_eq!(provider.catalog(), KIMI_CODE_CATALOG);
        assert!(valid_kimi_verification_url(
            "https://auth.kimi.com/device?code=abc"
        ));
        assert!(!valid_kimi_verification_url(
            "https://example.com/device?code=abc"
        ));
    }

    #[test]
    fn parses_kimi_token_response_without_leaking_secrets() {
        let token = parse_token_response(TokenResponse {
            access_token: "access-secret".to_owned(),
            refresh_token: "refresh-secret".to_owned(),
            expires_in: 3600,
            scope: Some("scope".to_owned()),
            token_type: Some("Bearer".to_owned()),
        })
        .unwrap();
        assert_eq!(token.access_token.expose(), "access-secret");
        assert_eq!(token.refresh_token.unwrap().expose(), "refresh-secret");
        let error = parse_token_response(TokenResponse {
            access_token: String::new(),
            refresh_token: "refresh-secret".to_owned(),
            expires_in: 3600,
            scope: None,
            token_type: None,
        })
        .unwrap_err()
        .to_string();
        assert!(!error.contains("refresh-secret"));
        assert!(!error.contains("access-secret"));
    }

    #[test]
    fn validates_qwen_endpoints() {
        for endpoint in [
            "https://dashscope-us.aliyuncs.com/compatible-mode/v1",
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "https://workspace-1.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
            "https://abc123.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1/",
        ] {
            assert_eq!(
                validate_qwen_endpoint(endpoint).unwrap(),
                endpoint.trim_end_matches('/')
            );
        }
        for endpoint in [
            "http://dashscope-us.aliyuncs.com/compatible-mode/v1",
            "https://dashscope-us.aliyuncs.com.evil.test/compatible-mode/v1",
            "https://user@dashscope-us.aliyuncs.com/compatible-mode/v1",
            "https://dashscope-us.aliyuncs.com:444/compatible-mode/v1",
            "https://dashscope-us.aliyuncs.com/v1",
            "https://dashscope-us.aliyuncs.com/compatible-mode/v1?x=1",
            "https://workspace_1.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
            "https://workspace-1.cn-hangzhou.maas.aliyuncs.com/compatible-mode/v1",
        ] {
            assert!(
                validate_qwen_endpoint(endpoint).is_err(),
                "expected rejection for {endpoint}"
            );
        }
    }

    #[test]
    fn invalid_qwen_endpoint_does_not_authenticate() {
        let store = store("goat-provider-hosted-qwen-invalid.json");
        store
            .store(
                &CredentialKey::model(QWEN, "default"),
                Credential::ApiKeyWithEndpoint {
                    secret: SecretString::from("key".to_owned()),
                    endpoint: "https://example.com/compatible-mode/v1".to_owned(),
                },
            )
            .unwrap();
        let provider = build_qwen(&store, "default");
        assert!(!provider.authenticated());
    }

    #[test]
    fn qwen_endpoint_credential_authenticates() {
        let store = store("goat-provider-hosted-qwen-valid.json");
        store
            .store(
                &CredentialKey::model(QWEN, "default"),
                Credential::ApiKeyWithEndpoint {
                    secret: SecretString::from("key".to_owned()),
                    endpoint: "https://dashscope-us.aliyuncs.com/compatible-mode/v1".to_owned(),
                },
            )
            .unwrap();
        let provider = build_qwen(&store, "default");
        assert!(provider.authenticated());
        assert_eq!(
            provider.base_url(),
            "https://dashscope-us.aliyuncs.com/compatible-mode/v1"
        );
    }

    #[test]
    fn local_no_auth_behavior_is_not_host_checked() {
        let store = store("goat-provider-hosted-local.json");
        let provider = build_deepseek(&store, "default");
        assert_eq!(provider.capabilities().auth, AuthMethod::ApiKey);
    }
}
