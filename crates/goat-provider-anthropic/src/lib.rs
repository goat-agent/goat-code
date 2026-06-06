use eventsource_stream::Eventsource;
use futures::StreamExt;
use goat_auth::{CredentialKey, CredentialStore};
use goat_provider::{
    AuthMethod, MessageRole, ModelEvent, ModelInfo, ModelProvider, ModelRequest,
    ProviderCapabilities, ProviderId,
};
use serde::{Deserialize, Serialize};
use tokio::{sync::mpsc, task::JoinHandle};

pub const PROVIDER_ID: &str = "anthropic";
const BASE_URL: &str = "https://api.anthropic.com/v1";
const ENV_VAR: &str = "ANTHROPIC_API_KEY";
const VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 4096;

const CATALOG: &[&str] = &[
    "claude-opus-4-8",
    "claude-sonnet-4-6",
    "claude-haiku-4-5-20251001",
    "claude-opus-4-7",
    "claude-opus-4-6",
    "claude-opus-4-5-20251101",
    "claude-sonnet-4-5-20250929",
    "claude-opus-4-1-20250805",
];

pub struct AnthropicProvider {
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(api_key: Option<String>) -> Self {
        Self {
            base_url: BASE_URL.to_owned(),
            api_key,
            client: reqwest::Client::new(),
        }
    }
}

pub fn build(store: &CredentialStore, account: &str) -> AnthropicProvider {
    let key = CredentialKey {
        provider: PROVIDER_ID.to_owned(),
        account: account.to_owned(),
    };
    let api_key = store
        .resolve(&key, Some(ENV_VAR))
        .map(|cred| cred.bearer().to_owned());
    AnthropicProvider::new(api_key)
}

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    messages: Vec<OutMessage<'a>>,
}

#[derive(Serialize)]
struct OutMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct DeltaEvent {
    delta: Option<TextDelta>,
}

#[derive(Deserialize)]
struct TextDelta {
    text: Option<String>,
}

#[derive(Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelDto>,
}

#[derive(Deserialize)]
struct ModelDto {
    id: String,
}

fn parse_text_delta(data: &str) -> Option<String> {
    serde_json::from_str::<DeltaEvent>(data).ok()?.delta?.text
}

impl ModelProvider for AnthropicProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from(PROVIDER_ID)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            tools: false,
            auth: AuthMethod::ApiKey,
        }
    }

    fn request(&self, req: ModelRequest, events: mpsc::Sender<ModelEvent>) -> JoinHandle<()> {
        let client = self.client.clone();
        let url = format!("{}/messages", self.base_url);
        let api_key = self.api_key.clone();
        tokio::spawn(async move {
            let mut system = String::new();
            let mut out_messages = Vec::new();
            for message in &req.messages {
                match message.role {
                    MessageRole::System => {
                        if !system.is_empty() {
                            system.push('\n');
                        }
                        system.push_str(&message.content);
                    }
                    MessageRole::User => out_messages.push(OutMessage {
                        role: "user",
                        content: &message.content,
                    }),
                    MessageRole::Assistant => out_messages.push(OutMessage {
                        role: "assistant",
                        content: &message.content,
                    }),
                }
            }
            let body = MessagesRequest {
                model: &req.model,
                max_tokens: MAX_TOKENS,
                stream: true,
                system: if system.is_empty() {
                    None
                } else {
                    Some(system.as_str())
                },
                messages: out_messages,
            };
            let mut builder = client
                .post(&url)
                .header("anthropic-version", VERSION)
                .json(&body);
            if let Some(key) = &api_key {
                builder = builder.header("x-api-key", key);
            }
            let resp = match builder.send().await {
                Ok(resp) => resp,
                Err(err) => {
                    let _ = events
                        .send(ModelEvent::Failed {
                            message: err.to_string(),
                        })
                        .await;
                    return;
                }
            };
            if !resp.status().is_success() {
                let status = resp.status();
                let detail = resp.text().await.unwrap_or_default();
                let _ = events
                    .send(ModelEvent::Failed {
                        message: format!("{status}: {detail}"),
                    })
                    .await;
                return;
            }
            let mut stream = resp.bytes_stream().eventsource();
            while let Some(event) = stream.next().await {
                match event {
                    Ok(event) => match event.event.as_str() {
                        "content_block_delta" => {
                            if let Some(text) = parse_text_delta(&event.data)
                                && events.send(ModelEvent::TextDelta { text }).await.is_err()
                            {
                                return;
                            }
                        }
                        "message_stop" => break,
                        "error" => {
                            let _ = events
                                .send(ModelEvent::Failed {
                                    message: event.data,
                                })
                                .await;
                            return;
                        }
                        _ => {}
                    },
                    Err(err) => {
                        let _ = events
                            .send(ModelEvent::Failed {
                                message: err.to_string(),
                            })
                            .await;
                        return;
                    }
                }
            }
            let _ = events.send(ModelEvent::Completed).await;
        })
    }

    fn authenticated(&self) -> bool {
        self.api_key.is_some()
    }

    fn catalog(&self) -> &'static [&'static str] {
        CATALOG
    }

    fn discover(&self, out: mpsc::Sender<ModelInfo>) -> JoinHandle<()> {
        let client = self.client.clone();
        let url = format!("{}/models", self.base_url);
        let api_key = self.api_key.clone();
        tokio::spawn(async move {
            let mut builder = client.get(&url).header("anthropic-version", VERSION);
            if let Some(key) = &api_key {
                builder = builder.header("x-api-key", key);
            }
            let Ok(resp) = builder.send().await else {
                return;
            };
            let Ok(models) = resp.json::<ModelsResponse>().await else {
                return;
            };
            for model in models.data {
                if out.send(ModelInfo { id: model.id }).await.is_err() {
                    return;
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::parse_text_delta;

    #[test]
    fn parses_text_delta() {
        let data = r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hi"}}"#;
        assert_eq!(parse_text_delta(data).as_deref(), Some("Hi"));
    }

    #[test]
    fn ignores_non_text_delta() {
        let data = r#"{"type":"content_block_delta","delta":{"type":"input_json_delta"}}"#;
        assert_eq!(parse_text_delta(data), None);
    }

    #[test]
    fn handles_malformed_json() {
        assert_eq!(parse_text_delta("not json"), None);
    }
}
