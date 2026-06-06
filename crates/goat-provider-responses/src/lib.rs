use eventsource_stream::Eventsource;
use futures::StreamExt;
use goat_provider::{
    AuthMethod, MessageRole, ModelEvent, ModelInfo, ModelProvider, ModelRequest,
    ProviderCapabilities, ProviderId, ProviderMessage,
};
use serde::{Deserialize, Serialize};
use tokio::{sync::mpsc, task::JoinHandle};

#[derive(Serialize)]
struct ResponsesRequest<'a> {
    model: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'a str>,
    input: Vec<InputMessage<'a>>,
    tools: Vec<serde_json::Value>,
    tool_choice: &'a str,
    parallel_tool_calls: bool,
    store: bool,
    stream: bool,
}

#[derive(Serialize)]
struct InputMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    role: &'static str,
    content: [ContentPart<'a>; 1],
}

#[derive(Serialize)]
struct ContentPart<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    text: &'a str,
}

pub fn build_body(
    model: &str,
    messages: &[ProviderMessage],
    default_instructions: Option<&str>,
    store: bool,
) -> serde_json::Value {
    let mut instructions = String::new();
    let mut input: Vec<InputMessage> = Vec::new();
    for message in messages {
        match message.role {
            MessageRole::System => {
                if !instructions.is_empty() {
                    instructions.push('\n');
                }
                instructions.push_str(&message.content);
            }
            MessageRole::User => input.push(InputMessage {
                kind: "message",
                role: "user",
                content: [ContentPart {
                    kind: "input_text",
                    text: &message.content,
                }],
            }),
            MessageRole::Assistant => input.push(InputMessage {
                kind: "message",
                role: "assistant",
                content: [ContentPart {
                    kind: "output_text",
                    text: &message.content,
                }],
            }),
        }
    }
    let instructions = if instructions.is_empty() {
        default_instructions
    } else {
        Some(instructions.as_str())
    };
    let request = ResponsesRequest {
        model,
        instructions,
        input,
        tools: Vec::new(),
        tool_choice: "auto",
        parallel_tool_calls: false,
        store,
        stream: true,
    };
    serde_json::to_value(request).unwrap_or_default()
}

pub async fn run_request(
    client: &reqwest::Client,
    url: &str,
    bearer: Option<&str>,
    account_id: Option<&str>,
    body: &serde_json::Value,
    events: &mpsc::Sender<ModelEvent>,
) {
    let mut builder = client
        .post(url)
        .header("Accept", "text/event-stream")
        .json(body);
    if let Some(token) = bearer {
        builder = builder.bearer_auth(token);
    }
    if let Some(account) = account_id {
        builder = builder.header("chatgpt-account-id", account);
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
    stream_responses(resp, events).await;
}

async fn stream_responses(response: reqwest::Response, events: &mpsc::Sender<ModelEvent>) {
    let mut stream = response.bytes_stream().eventsource();
    while let Some(event) = stream.next().await {
        match event {
            Ok(event) => match event.event.as_str() {
                "response.output_text.delta" => {
                    if let Some(text) = parse_output_delta(&event.data)
                        && events.send(ModelEvent::TextDelta { text }).await.is_err()
                    {
                        return;
                    }
                }
                "response.completed" => break,
                "response.failed" | "error" => {
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
}

#[derive(Deserialize)]
struct OutputTextDelta {
    delta: Option<String>,
}

fn parse_output_delta(data: &str) -> Option<String> {
    serde_json::from_str::<OutputTextDelta>(data).ok()?.delta
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

pub struct ResponsesProvider {
    id: ProviderId,
    base_url: String,
    bearer: Option<String>,
    auth: AuthMethod,
    client: reqwest::Client,
    model_filter: Option<fn(&str) -> bool>,
    catalog: &'static [&'static str],
}

impl ResponsesProvider {
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
            model_filter: None,
            catalog: &[],
        }
    }

    #[must_use]
    pub fn with_model_filter(mut self, filter: fn(&str) -> bool) -> Self {
        self.model_filter = Some(filter);
        self
    }

    #[must_use]
    pub fn with_catalog(mut self, catalog: &'static [&'static str]) -> Self {
        self.catalog = catalog;
        self
    }
}

impl ModelProvider for ResponsesProvider {
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

    fn catalog(&self) -> &'static [&'static str] {
        self.catalog
    }

    fn request(&self, req: ModelRequest, events: mpsc::Sender<ModelEvent>) -> JoinHandle<()> {
        let client = self.client.clone();
        let url = format!("{}/responses", self.base_url);
        let bearer = self.bearer.clone();
        tokio::spawn(async move {
            let body = build_body(&req.model, &req.messages, None, false);
            run_request(&client, &url, bearer.as_deref(), None, &body, &events).await;
        })
    }

    fn discover(&self, out: mpsc::Sender<ModelInfo>) -> JoinHandle<()> {
        let client = self.client.clone();
        let url = format!("{}/models", self.base_url);
        let bearer = self.bearer.clone();
        let filter = self.model_filter;
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
                if let Some(keep) = filter
                    && !keep(&model.id)
                {
                    continue;
                }
                if out.send(ModelInfo { id: model.id }).await.is_err() {
                    return;
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{build_body, parse_output_delta};
    use goat_provider::{MessageRole, ProviderMessage};

    #[test]
    fn parses_output_text_delta() {
        let data = r#"{"type":"response.output_text.delta","delta":"Hi"}"#;
        assert_eq!(parse_output_delta(data).as_deref(), Some("Hi"));
    }

    #[test]
    fn default_instructions_used_when_no_system_message() {
        let messages = vec![ProviderMessage {
            role: MessageRole::User,
            content: "hi".to_owned(),
        }];
        let body = build_body("gpt-5.5", &messages, Some("base"), false);
        assert_eq!(body["instructions"], "base");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
    }

    #[test]
    fn system_message_overrides_default_instructions() {
        let messages = vec![
            ProviderMessage {
                role: MessageRole::System,
                content: "be terse".to_owned(),
            },
            ProviderMessage {
                role: MessageRole::User,
                content: "hi".to_owned(),
            },
        ];
        let body = build_body("gpt-5.5", &messages, Some("base"), false);
        assert_eq!(body["instructions"], "be terse");
    }

    #[test]
    fn instructions_omitted_when_empty_and_no_default() {
        let messages = vec![ProviderMessage {
            role: MessageRole::User,
            content: "hi".to_owned(),
        }];
        let body = build_body("gpt-5.5", &messages, None, false);
        assert!(body.get("instructions").is_none());
    }
}
