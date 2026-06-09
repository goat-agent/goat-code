mod codeassist;
mod oauth;
mod wire;

use std::sync::Arc;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use goat_auth::{CredentialKey, CredentialStore, TokenSet};
use goat_provider::{
    AuthMethod, Capabilities, Effort, Model, Provider, ProviderId, Request, StreamEvent,
};
use tokio::{sync::Mutex, sync::mpsc, task::JoinHandle};

pub const PROVIDER_ID: &str = "gemini";
pub const ENV_VAR: &str = "GEMINI_API_KEY";
const GL_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

const CATALOG: &[&str] = &[
    "gemini-3.5-flash",
    "gemini-3.1-pro-preview",
    "gemini-3.1-flash-lite",
    "gemini-2.5-pro",
    "gemini-2.5-flash",
    "gemini-2.5-flash-lite",
];

pub struct GeminiProvider {
    store: CredentialStore,
    key: CredentialKey,
    client: reqwest::Client,
    project: Arc<Mutex<Option<String>>>,
}

impl GeminiProvider {
    fn new(store: CredentialStore, key: CredentialKey) -> Self {
        Self {
            store,
            key,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_mins(5))
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("reqwest client"),
            project: Arc::new(Mutex::new(None)),
        }
    }
}

pub fn build(store: &CredentialStore, account: &str) -> GeminiProvider {
    let key = CredentialKey {
        provider: PROVIDER_ID.to_owned(),
        account: account.to_owned(),
    };
    GeminiProvider::new(store.clone(), key)
}

#[derive(serde::Deserialize)]
struct GlModelsResponse {
    #[serde(default)]
    models: Vec<GlModel>,
}

#[derive(serde::Deserialize)]
struct GlModel {
    name: String,
    #[serde(default, rename = "supportedGenerationMethods")]
    supported_generation_methods: Vec<String>,
}

async fn fetch_gl_models(client: &reqwest::Client, api_key: &str) -> Vec<Model> {
    let url = format!("{GL_BASE}/models");
    let Ok(resp) = client
        .get(&url)
        .header("x-goog-api-key", api_key)
        .send()
        .await
    else {
        return Vec::new();
    };
    if !resp.status().is_success() {
        return Vec::new();
    }
    let Ok(body) = resp.json::<GlModelsResponse>().await else {
        return Vec::new();
    };
    body.models
        .into_iter()
        .filter(|m| {
            m.supported_generation_methods
                .iter()
                .any(|method| method == "generateContent")
        })
        .map(|m| Model {
            id: m.name.strip_prefix("models/").unwrap_or(&m.name).to_owned(),
        })
        .collect()
}

async fn stream_response(resp: reqwest::Response, tx: &mpsc::Sender<StreamEvent>, oauth: bool) {
    let mut stream = resp.bytes_stream().eventsource();
    while let Some(event) = stream.next().await {
        match event {
            Ok(event) => {
                if event.data == "[DONE]" {
                    break;
                }
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&event.data) else {
                    continue;
                };
                if let Some(reason) = wire::extract_finish_reason(&value, oauth) {
                    let finished = matches!(reason, "STOP" | "MAX_TOKENS");
                    for ev in wire::parse_chunk(&value, oauth) {
                        if tx.send(ev).await.is_err() {
                            return;
                        }
                    }
                    if finished {
                        break;
                    }
                } else {
                    for ev in wire::parse_chunk(&value, oauth) {
                        if tx.send(ev).await.is_err() {
                            return;
                        }
                    }
                }
            }
            Err(err) => {
                let _ = tx
                    .send(StreamEvent::Failed {
                        message: err.to_string(),
                    })
                    .await;
                return;
            }
        }
    }
    let _ = tx.send(StreamEvent::Completed).await;
}

impl Provider for GeminiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from(PROVIDER_ID)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tools: true,
            auth: AuthMethod::ApiKeyOrOAuth,
        }
    }

    fn authenticated(&self) -> bool {
        self.store.resolve(&self.key, Some(ENV_VAR)).is_some()
    }

    fn catalog(&self) -> &'static [&'static str] {
        CATALOG
    }

    fn efforts(&self, model: &str) -> Vec<Effort> {
        wire::gemini_efforts(model)
    }

    fn validate(&self) -> JoinHandle<Result<(), String>> {
        let store = self.store.clone();
        let key = self.key.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            let auth = oauth::current_auth(&store, &key)
                .await
                .ok_or_else(|| "no credentials".to_owned())?;
            let api_key = match auth {
                oauth::Auth::OAuth(_) => return Ok(()),
                oauth::Auth::ApiKey(k) => k,
            };
            let url = format!("{GL_BASE}/models");
            let resp = client
                .get(&url)
                .header("x-goog-api-key", &api_key)
                .send()
                .await
                .map_err(|_| "could not reach Gemini API".to_owned())?;
            let status = resp.status();
            if status.is_success() {
                Ok(())
            } else if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                Err("invalid API key".to_owned())
            } else {
                Err(format!("could not reach Gemini API: {status}"))
            }
        })
    }

    fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
        let client = self.client.clone();
        let store = self.store.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let Some(auth) = oauth::current_auth(&store, &key).await else {
                return;
            };
            match auth {
                oauth::Auth::OAuth(_) => {
                    for &id in CATALOG {
                        if out.send(Model { id: id.to_owned() }).await.is_err() {
                            return;
                        }
                    }
                }
                oauth::Auth::ApiKey(api_key) => {
                    for model in fetch_gl_models(&client, &api_key).await {
                        if out.send(model).await.is_err() {
                            return;
                        }
                    }
                }
            }
        })
    }

    fn stream(&self, req: Request, tx: mpsc::Sender<StreamEvent>) -> JoinHandle<()> {
        let client = self.client.clone();
        let store = self.store.clone();
        let key = self.key.clone();
        let project_cache = Arc::clone(&self.project);
        tokio::spawn(async move {
            let Some(auth) = oauth::current_auth(&store, &key).await else {
                let _ = tx
                    .send(StreamEvent::Failed {
                        message: "not logged in to gemini".to_owned(),
                    })
                    .await;
                return;
            };

            let inner = wire::build_request(&req);
            let inner_value = wire::inner_request_to_value(&inner);

            tracing::debug!(model = %req.model, body = %inner_value, "gemini request");

            let (builder, oauth) = match &auth {
                oauth::Auth::ApiKey(api_key) => {
                    let url = format!(
                        "{GL_BASE}/models/{}:streamGenerateContent?alt=sse",
                        req.model
                    );
                    tracing::debug!(%url, "gemini api-key stream");
                    let b = client
                        .post(&url)
                        .header("x-goog-api-key", api_key)
                        .json(&inner_value);
                    (b, false)
                }
                oauth::Auth::OAuth(access) => {
                    let project =
                        match codeassist::resolve_project(&client, access, &project_cache).await {
                            Ok(p) => p,
                            Err(e) => {
                                let _ = tx.send(StreamEvent::Failed { message: e }).await;
                                return;
                            }
                        };
                    let body =
                        codeassist::wrap_request(&req.model, project.as_deref(), &inner_value);
                    let url = format!("{}:streamGenerateContent?alt=sse", codeassist::CA_BASE);
                    let b = client.post(&url).bearer_auth(access).json(&body);
                    (b, true)
                }
            };

            let resp = match builder.send().await {
                Ok(r) => r,
                Err(err) => {
                    let _ = tx
                        .send(StreamEvent::Failed {
                            message: err.to_string(),
                        })
                        .await;
                    return;
                }
            };

            if !resp.status().is_success() {
                let status = resp.status();
                let detail = resp.text().await.unwrap_or_default();
                let _ = tx
                    .send(StreamEvent::Failed {
                        message: format!("{status}: {detail}"),
                    })
                    .await;
                return;
            }

            stream_response(resp, &tx, oauth).await;
        })
    }

    fn login(&self, status: mpsc::Sender<String>) -> JoinHandle<Result<TokenSet, String>> {
        tokio::spawn(async move { oauth::do_login(&status).await.map_err(|e| e.to_string()) })
    }
}
