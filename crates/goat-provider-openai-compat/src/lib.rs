use eventsource_stream::Eventsource;
use futures::StreamExt;
use goat_provider::{
    AuthMethod, ModelEvent, ModelInfo, ModelProvider, ModelRequest, ProviderCapabilities,
    ProviderId, ProviderMessage,
};
use serde::{Deserialize, Serialize};
use tokio::{sync::mpsc, task::JoinHandle};

pub struct OpenAiCompatProvider {
    id: ProviderId,
    base_url: String,
    bearer: Option<String>,
    auth: AuthMethod,
    client: reqwest::Client,
}

impl OpenAiCompatProvider {
    pub fn new(
        id: ProviderId,
        base_url: impl Into<String>,
        bearer: Option<String>,
        auth: AuthMethod,
    ) -> Self {
        Self {
            id,
            base_url: base_url.into(),
            bearer,
            auth,
            client: reqwest::Client::new(),
        }
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ProviderMessage],
    stream: bool,
}

#[derive(Deserialize)]
struct ChatChunk {
    #[serde(default)]
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    #[serde(default)]
    delta: ChatDelta,
}

#[derive(Default, Deserialize)]
struct ChatDelta {
    content: Option<String>,
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

fn parse_delta(data: &str) -> Option<String> {
    let chunk: ChatChunk = serde_json::from_str(data).ok()?;
    chunk.choices.into_iter().next()?.delta.content
}

impl ModelProvider for OpenAiCompatProvider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    fn authenticated(&self) -> bool {
        match self.auth {
            AuthMethod::None => true,
            _ => self.bearer.is_some(),
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            tools: false,
            auth: self.auth,
        }
    }

    fn request(&self, req: ModelRequest, events: mpsc::Sender<ModelEvent>) -> JoinHandle<()> {
        let client = self.client.clone();
        let url = format!("{}/chat/completions", self.base_url);
        let bearer = self.bearer.clone();
        tokio::spawn(async move {
            let body = ChatRequest {
                model: &req.model,
                messages: &req.messages,
                stream: true,
            };
            let mut builder = client.post(&url).json(&body);
            if let Some(token) = &bearer {
                builder = builder.bearer_auth(token);
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
                    Ok(event) => {
                        if event.data == "[DONE]" {
                            break;
                        }
                        if let Some(text) = parse_delta(&event.data)
                            && events.send(ModelEvent::TextDelta { text }).await.is_err()
                        {
                            return;
                        }
                    }
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

    fn discover(&self, out: mpsc::Sender<ModelInfo>) -> JoinHandle<()> {
        let client = self.client.clone();
        let url = format!("{}/models", self.base_url);
        let bearer = self.bearer.clone();
        tokio::spawn(async move {
            let mut builder = client.get(&url);
            if let Some(token) = &bearer {
                builder = builder.bearer_auth(token);
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
    use super::parse_delta;

    #[test]
    fn parses_content_delta() {
        let data = r#"{"choices":[{"delta":{"content":"Hello"}}]}"#;
        assert_eq!(parse_delta(data).as_deref(), Some("Hello"));
    }

    #[test]
    fn ignores_empty_delta() {
        assert_eq!(parse_delta(r#"{"choices":[{"delta":{}}]}"#), None);
    }

    #[test]
    fn ignores_role_only_delta() {
        assert_eq!(
            parse_delta(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#),
            None
        );
    }

    #[test]
    fn handles_malformed_json() {
        assert_eq!(parse_delta("not json"), None);
    }
}
