mod oauth;

use goat_auth::{Credential, CredentialKey, CredentialStore, TokenSet};
use goat_provider::{
    AuthMethod, Capabilities, Effort, Model, Provider, ProviderId, ProviderMetadata, Request,
    StreamError, StreamEvent, WebSearchOutput,
};
use goat_provider_openai_compat::{
    OpenAiCompatProvider, ResponsesProvider, enforce_https_host, no_efforts,
};
use tokio::{sync::mpsc, task::JoinHandle};

pub const PROVIDER_ID: &str = "xai";

const BASE_URL: &str = "https://api.x.ai/v1";
const ALLOWED_HOST: &str = "api.x.ai";

const SETUP: &[&str] = &[
    "xAI Grok provider (API key or SuperGrok / X Premium+ OAuth).",
    "API key: `goat provider login xai --key xai-...` or `XAI_API_KEY`.",
    "OAuth: `goat provider login xai` (browser or device code; no API key).",
];

const OAUTH_CATALOG: &[&str] = &[
    "grok-4.3",
    "grok-build-0.1",
    "grok-4.20-beta-latest-reasoning",
    "grok-4.20-beta-latest-non-reasoning",
];

const API_KEY_CATALOG: &[&str] = &[
    "grok-4",
    "grok-4-fast-reasoning",
    "grok-4-fast-non-reasoning",
    "grok-3",
    "grok-3-fast",
    "grok-3-mini",
];

const CATALOG: &[&str] = &[
    "grok-4.3",
    "grok-build-0.1",
    "grok-4.20-beta-latest-reasoning",
    "grok-4.20-beta-latest-non-reasoning",
    "grok-4",
    "grok-4-fast-reasoning",
    "grok-4-fast-non-reasoning",
    "grok-3",
    "grok-3-fast",
    "grok-3-mini",
];

const OAUTH_CONTEXT: &[(&str, u32)] = &[
    ("grok-4.3", 1_000_000),
    ("grok-build", 512_000),
    ("grok-4.20", 2_000_000),
];

const API_KEY_CONTEXT: &[(&str, u32)] = &[("grok-4", 256_000), ("grok-3", 131_072)];

pub fn build(store: &CredentialStore, account: &str) -> XaiProvider {
    enforce_https_host(BASE_URL, ALLOWED_HOST).expect("xai provider base URL");
    XaiProvider::new(store.clone(), CredentialKey::model(PROVIDER_ID, account))
}

enum XaiAuth {
    ApiKey(String),
    OAuth(String),
}

pub struct XaiProvider {
    store: CredentialStore,
    key: CredentialKey,
}

impl XaiProvider {
    pub fn new(store: CredentialStore, key: CredentialKey) -> Self {
        Self { store, key }
    }

    async fn resolve_auth(&self) -> Option<XaiAuth> {
        let cred = self.store.resolve(&self.key, Some("XAI_API_KEY"))?;
        match cred {
            Credential::ApiKey(secret) | Credential::ApiKeyWithEndpoint { secret, .. } => {
                Some(XaiAuth::ApiKey(secret.expose().to_owned()))
            }
            Credential::OAuth(_) => oauth::current_oauth_token(&self.store, &self.key)
                .await
                .map(XaiAuth::OAuth),
        }
    }

    fn is_oauth_model(model: &str) -> bool {
        OAUTH_CATALOG
            .iter()
            .any(|id| *id == model || model.starts_with(id))
    }

    fn chat_provider(bearer: String) -> OpenAiCompatProvider {
        OpenAiCompatProvider::new(
            ProviderId::from(PROVIDER_ID),
            BASE_URL,
            Some(bearer),
            AuthMethod::ApiKeyOrOAuth,
        )
        .with_catalog(API_KEY_CATALOG)
        .with_context_windows(API_KEY_CONTEXT)
        .with_vision_filter(vision_model)
        .with_efforts(no_efforts)
        .with_reasoning_effort(false)
    }

    fn responses_provider(bearer: String) -> ResponsesProvider {
        ResponsesProvider::new(
            ProviderId::from(PROVIDER_ID),
            BASE_URL,
            Some(bearer),
            AuthMethod::ApiKeyOrOAuth,
        )
        .with_catalog(OAUTH_CATALOG)
        .with_context_windows(OAUTH_CONTEXT)
        .with_vision_filter(vision_model)
        .with_model_filter(oauth_chat_model)
    }
}

impl Provider for XaiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from(PROVIDER_ID)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tools: true,
            auth: AuthMethod::ApiKeyOrOAuth,
            images: true,
        }
    }

    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            env_var: Some("XAI_API_KEY"),
            validation: "network",
            endpoint: None,
            oauth: Some("browser or device code (SuperGrok / X Premium+)"),
            login_endpoint: None,
            setup: SETUP,
        }
    }

    fn authenticated(&self) -> bool {
        self.store.resolve(&self.key, Some("XAI_API_KEY")).is_some()
    }

    fn catalog(&self) -> &'static [&'static str] {
        CATALOG
    }

    fn efforts(&self, model: &str) -> Vec<Effort> {
        if Self::is_oauth_model(model) {
            oauth_efforts(model)
        } else {
            no_efforts(model)
        }
    }

    fn context_window(&self, model: &str) -> Option<u32> {
        if Self::is_oauth_model(model) {
            OAUTH_CONTEXT
                .iter()
                .find_map(|(prefix, window)| model.starts_with(prefix).then_some(*window))
        } else {
            API_KEY_CONTEXT
                .iter()
                .find_map(|(prefix, window)| model.starts_with(prefix).then_some(*window))
        }
    }

    fn supports_images(&self, model: &str) -> bool {
        vision_model(model)
    }

    fn verifies_credentials(&self) -> bool {
        true
    }

    fn validate(&self) -> JoinHandle<Result<(), String>> {
        let store = self.store.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let provider = XaiProvider { store, key };
            let Some(auth) = provider.resolve_auth().await else {
                return Err("no credentials".to_owned());
            };
            match auth {
                XaiAuth::ApiKey(bearer) => {
                    let handle = XaiProvider::chat_provider(bearer).validate();
                    handle.await.unwrap_or(Err("validation failed".to_owned()))
                }
                XaiAuth::OAuth(bearer) => {
                    let handle = XaiProvider::responses_provider(bearer).validate();
                    handle.await.unwrap_or(Err("validation failed".to_owned()))
                }
            }
        })
    }

    fn stream(&self, req: Request, events: mpsc::Sender<StreamEvent>) -> JoinHandle<()> {
        let store = self.store.clone();
        let key = self.key.clone();
        let model = req.model.clone();
        tokio::spawn(async move {
            let provider = XaiProvider { store, key };
            let Some(auth) = provider.resolve_auth().await else {
                let _ = events
                    .send(StreamEvent::Failed {
                        error: StreamError::auth("no credentials"),
                    })
                    .await;
                return;
            };
            let handle = match auth {
                XaiAuth::ApiKey(bearer) => XaiProvider::chat_provider(bearer).stream(req, events),
                XaiAuth::OAuth(bearer) => {
                    if !XaiProvider::is_oauth_model(&model)
                        && API_KEY_CATALOG.contains(&model.as_str())
                    {
                        let _ = events
                            .send(StreamEvent::Failed {
                                error: StreamError::invalid_request(format!(
                                    "model {model} requires an xAI API key; OAuth supports {}",
                                    OAUTH_CATALOG.join(", ")
                                )),
                            })
                            .await;
                        return;
                    }
                    XaiProvider::responses_provider(bearer).stream(req, events)
                }
            };
            let _ = handle.await;
        })
    }

    fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
        let store = self.store.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let provider = XaiProvider { store, key };
            let Some(auth) = provider.resolve_auth().await else {
                for id in CATALOG {
                    if out
                        .send(Model {
                            id: (*id).to_owned(),
                            supports_images: vision_model(id),
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                return;
            };
            let handle = match auth {
                XaiAuth::ApiKey(bearer) => XaiProvider::chat_provider(bearer).discover(out),
                XaiAuth::OAuth(bearer) => XaiProvider::responses_provider(bearer).discover(out),
            };
            let _ = handle.await;
        })
    }

    fn login(&self, status: mpsc::Sender<String>) -> JoinHandle<Result<TokenSet, String>> {
        tokio::spawn(async move { oauth::login(&status).await.map_err(|err| err.to_string()) })
    }

    fn web_search(&self, query: String) -> JoinHandle<Result<WebSearchOutput, StreamError>> {
        let _ = query;
        tokio::spawn(async { Err(StreamError::other("web search is not supported")) })
    }
}

fn oauth_chat_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    !(id.contains("embedding")
        || id.contains("tts")
        || id.contains("whisper")
        || id.contains("image")
        || id.contains("video"))
}

fn oauth_efforts(model: &str) -> Vec<Effort> {
    let id = model.to_ascii_lowercase();
    if id.starts_with("grok-4.3") {
        vec![Effort::Low, Effort::Medium, Effort::High]
    } else {
        Vec::new()
    }
}

fn vision_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id.starts_with("grok-4") || id.contains("vision")
}

#[cfg(test)]
mod tests {
    use goat_auth::CredentialStore;
    use goat_provider::{AuthMethod, Effort, Provider};

    use super::*;

    fn store(name: &str) -> CredentialStore {
        let _ = std::fs::remove_file(std::env::temp_dir().join(name));
        CredentialStore::new(std::env::temp_dir().join(name))
    }

    #[test]
    fn xai_supports_api_key_and_oauth() {
        let store = store("goat-provider-xai.json");
        let provider = build(&store, "default");
        assert_eq!(provider.capabilities().auth, AuthMethod::ApiKeyOrOAuth);
        assert_eq!(
            provider.metadata().oauth,
            Some("browser or device code (SuperGrok / X Premium+)")
        );
        assert!(!provider.authenticated());
        assert_eq!(provider.catalog(), CATALOG);
        assert_eq!(provider.context_window("grok-4.3"), Some(1_000_000));
        assert_eq!(provider.context_window("grok-4"), Some(256_000));
        assert_eq!(
            provider.efforts("grok-4.3"),
            vec![Effort::Low, Effort::Medium, Effort::High]
        );
    }
}
