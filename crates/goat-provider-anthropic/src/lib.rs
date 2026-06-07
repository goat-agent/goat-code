use std::collections::HashMap;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use goat_auth::{CredentialKey, CredentialStore};
use goat_provider::{
    AuthMethod, ContentBlock, Effort, MessageRole, ModelEvent, ModelInfo, ModelProvider,
    ModelRequest, ProviderCapabilities, ProviderId, ProviderMessage,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{sync::mpsc, task::JoinHandle};

pub const PROVIDER_ID: &str = "anthropic";
const BASE_URL: &str = "https://api.anthropic.com/v1";
const ENV_VAR: &str = "ANTHROPIC_API_KEY";
const VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 16384;

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
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_mins(5))
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("reqwest client"),
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
    messages: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<serde_json::Value>,
}

struct ThinkingConfig {
    thinking: Option<serde_json::Value>,
    output_config: Option<serde_json::Value>,
    max_tokens: u32,
}

fn uses_effort_param(model: &str) -> bool {
    let id = model.to_ascii_lowercase();
    id.contains("opus-4-8")
        || id.contains("opus-4-7")
        || id.contains("opus-4-6")
        || id.contains("sonnet-4-6")
}

fn budget_tokens(effort: Effort) -> Option<u32> {
    match effort {
        Effort::Off => None,
        Effort::Low => Some(2048),
        Effort::Medium => Some(8192),
        Effort::High | Effort::Xhigh | Effort::Max => Some(24576),
    }
}

fn thinking_config(model: &str, effort: Option<Effort>) -> ThinkingConfig {
    let none = ThinkingConfig {
        thinking: None,
        output_config: None,
        max_tokens: MAX_TOKENS,
    };
    let Some(effort) = effort else {
        return none;
    };
    if uses_effort_param(model) {
        if matches!(effort, Effort::Off) {
            return none;
        }
        ThinkingConfig {
            thinking: Some(json!({ "type": "adaptive" })),
            output_config: Some(json!({ "effort": effort.as_str() })),
            max_tokens: MAX_TOKENS,
        }
    } else {
        match budget_tokens(effort) {
            None => none,
            Some(budget) => ThinkingConfig {
                thinking: Some(json!({ "type": "enabled", "budget_tokens": budget })),
                output_config: None,
                max_tokens: budget + MAX_TOKENS,
            },
        }
    }
}

fn anthropic_efforts(model: &str) -> Vec<Effort> {
    let id = model.to_ascii_lowercase();
    if id.contains("opus-4-8") || id.contains("opus-4-7") {
        vec![
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::Xhigh,
            Effort::Max,
        ]
    } else if id.contains("opus-4-6") || id.contains("sonnet-4-6") {
        vec![Effort::Low, Effort::Medium, Effort::High, Effort::Max]
    } else if id.contains("opus-4-5")
        || id.contains("sonnet-4-5")
        || id.contains("opus-4-1")
        || id.contains("haiku-4-5")
    {
        vec![Effort::Off, Effort::Low, Effort::Medium, Effort::High]
    } else {
        Vec::new()
    }
}

fn content_block_json(block: &ContentBlock) -> serde_json::Value {
    match block {
        ContentBlock::Text { text } => json!({ "type": "text", "text": text }),
        ContentBlock::Thinking { text, signature } => json!({
            "type": "thinking",
            "thinking": text,
            "signature": signature,
        }),
        ContentBlock::RedactedThinking { data } => json!({
            "type": "redacted_thinking",
            "data": data,
        }),
        ContentBlock::ToolUse { id, name, input } => json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": input,
        }),
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": content,
            "is_error": is_error,
        }),
    }
}

fn message_json(role: &str, message: &ProviderMessage) -> serde_json::Value {
    let blocks: Vec<serde_json::Value> = message.content.iter().map(content_block_json).collect();
    json!({ "role": role, "content": blocks })
}

#[derive(Deserialize)]
struct DeltaEvent {
    delta: Option<DeltaBody>,
}

#[derive(Deserialize)]
struct DeltaBody {
    text: Option<String>,
    partial_json: Option<String>,
    thinking: Option<String>,
    signature: Option<String>,
}

#[derive(Deserialize)]
struct ContentBlockStart {
    index: u32,
    content_block: ContentBlockInfo,
}

#[derive(Deserialize)]
struct ContentBlockInfo {
    #[serde(rename = "type")]
    kind: String,
    id: Option<String>,
    name: Option<String>,
    data: Option<String>,
}

#[derive(Deserialize)]
struct ContentBlockStop {
    index: u32,
}

#[derive(Deserialize)]
struct IndexedEvent {
    index: u32,
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

fn parse_input_json_delta(data: &str) -> Option<String> {
    serde_json::from_str::<DeltaEvent>(data)
        .ok()?
        .delta?
        .partial_json
}

fn parse_thinking_delta(data: &str) -> Option<String> {
    serde_json::from_str::<DeltaEvent>(data)
        .ok()?
        .delta?
        .thinking
}

fn parse_signature_delta(data: &str) -> Option<String> {
    serde_json::from_str::<DeltaEvent>(data)
        .ok()?
        .delta?
        .signature
}

fn event_index(data: &str) -> Result<u32, serde_json::Error> {
    serde_json::from_str::<IndexedEvent>(data).map(|event| event.index)
}

fn split_request(req: &ModelRequest) -> (String, Vec<serde_json::Value>, Vec<serde_json::Value>) {
    let mut system = String::new();
    let mut messages = Vec::new();
    for message in &req.messages {
        match message.role {
            MessageRole::System => {
                if !system.is_empty() {
                    system.push('\n');
                }
                system.push_str(&message.text_content());
            }
            MessageRole::User => messages.push(message_json("user", message)),
            MessageRole::Assistant => messages.push(message_json("assistant", message)),
        }
    }
    let tools = req
        .tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.input_schema,
            })
        })
        .collect();
    (system, messages, tools)
}

async fn stream_messages(response: reqwest::Response, events: &mpsc::Sender<ModelEvent>) {
    let mut stream = response.bytes_stream().eventsource();
    let mut tool_calls: HashMap<u32, (String, String, String)> = HashMap::new();
    while let Some(event) = stream.next().await {
        match event {
            Ok(event) => match event.event.as_str() {
                "content_block_start" => {
                    if let Ok(start) = serde_json::from_str::<ContentBlockStart>(&event.data) {
                        match start.content_block.kind.as_str() {
                            "tool_use" => {
                                if let (Some(id), Some(name)) =
                                    (start.content_block.id, start.content_block.name)
                                {
                                    tool_calls.insert(start.index, (id, name, String::new()));
                                }
                            }
                            "redacted_thinking" => {
                                if let Some(data) = start.content_block.data
                                    && events
                                        .send(ModelEvent::RedactedThinking { data })
                                        .await
                                        .is_err()
                                {
                                    return;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "content_block_delta" => {
                    if let Some(text) = parse_text_delta(&event.data) {
                        if events.send(ModelEvent::TextDelta { text }).await.is_err() {
                            return;
                        }
                    } else if let Some(text) = parse_thinking_delta(&event.data) {
                        if events
                            .send(ModelEvent::ThinkingDelta { text })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    } else if let Some(signature) = parse_signature_delta(&event.data) {
                        if events
                            .send(ModelEvent::ThinkingSignature { signature })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    } else if let Some(partial) = parse_input_json_delta(&event.data)
                        && let Ok(index) = event_index(&event.data)
                        && let Some(entry) = tool_calls.get_mut(&index)
                    {
                        entry.2.push_str(&partial);
                    }
                }
                "content_block_stop" => {
                    if let Ok(stop) = serde_json::from_str::<ContentBlockStop>(&event.data)
                        && let Some((id, name, input)) = tool_calls.remove(&stop.index)
                        && events
                            .send(ModelEvent::ToolCall { id, name, input })
                            .await
                            .is_err()
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
}

impl ModelProvider for AnthropicProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from(PROVIDER_ID)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            tools: true,
            auth: AuthMethod::ApiKey,
        }
    }

    fn request(&self, req: ModelRequest, events: mpsc::Sender<ModelEvent>) -> JoinHandle<()> {
        let client = self.client.clone();
        let url = format!("{}/messages", self.base_url);
        let api_key = self.api_key.clone();
        tokio::spawn(async move {
            let (system, messages, tools) = split_request(&req);
            let cfg = thinking_config(&req.model, req.effort);
            let body = MessagesRequest {
                model: &req.model,
                max_tokens: cfg.max_tokens,
                stream: true,
                system: if system.is_empty() {
                    None
                } else {
                    Some(system.as_str())
                },
                messages,
                tools,
                thinking: cfg.thinking,
                output_config: cfg.output_config,
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
            stream_messages(resp, &events).await;
        })
    }

    fn authenticated(&self) -> bool {
        self.api_key.is_some()
    }

    fn validate(&self) -> JoinHandle<Result<(), String>> {
        let client = self.client.clone();
        let url = format!("{}/models", self.base_url);
        let api_key = self.api_key.clone();
        tokio::spawn(async move {
            let Some(key) = api_key else {
                return Err("no credentials".to_owned());
            };
            let resp = client
                .get(&url)
                .header("anthropic-version", VERSION)
                .header("x-api-key", key)
                .send()
                .await
                .map_err(|_| "could not reach provider".to_owned())?;
            let status = resp.status();
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

    fn catalog(&self) -> &'static [&'static str] {
        CATALOG
    }

    fn efforts(&self, model: &str) -> Vec<Effort> {
        anthropic_efforts(model)
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
    use std::collections::HashMap;

    use super::{event_index, parse_input_json_delta, parse_text_delta};

    #[test]
    fn parses_text_delta() {
        let data = r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hi"}}"#;
        assert_eq!(parse_text_delta(data).as_deref(), Some("Hi"));
    }

    #[test]
    fn text_delta_helper_skips_input_json() {
        let data = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"a\""}}"#;
        assert_eq!(parse_text_delta(data), None);
    }

    #[test]
    fn parses_input_json_delta() {
        let data = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"a\""}}"#;
        assert_eq!(parse_input_json_delta(data).as_deref(), Some("{\"a\""));
        assert_eq!(event_index(data).unwrap(), 1);
    }

    #[test]
    fn accumulates_tool_call_across_events() {
        let mut tool_calls: HashMap<u32, (String, String, String)> = HashMap::new();
        let start = r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file"}}"#;
        let start: super::ContentBlockStart = serde_json::from_str(start).unwrap();
        assert_eq!(start.content_block.kind, "tool_use");
        tool_calls.insert(
            start.index,
            (
                start.content_block.id.unwrap(),
                start.content_block.name.unwrap(),
                String::new(),
            ),
        );

        let first = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#;
        let second = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"a.txt\"}"}}"#;
        for chunk in [first, second] {
            let partial = parse_input_json_delta(chunk).unwrap();
            let index = event_index(chunk).unwrap();
            tool_calls.get_mut(&index).unwrap().2.push_str(&partial);
        }

        let stop = r#"{"type":"content_block_stop","index":0}"#;
        let stop: super::ContentBlockStop = serde_json::from_str(stop).unwrap();
        let (id, name, input) = tool_calls.remove(&stop.index).unwrap();
        assert_eq!(id, "toolu_1");
        assert_eq!(name, "read_file");
        assert_eq!(input, r#"{"path":"a.txt"}"#);
    }

    #[test]
    fn handles_malformed_json() {
        assert_eq!(parse_text_delta("not json"), None);
        assert_eq!(parse_input_json_delta("not json"), None);
    }

    #[test]
    fn serializes_thinking_blocks() {
        use goat_provider::ContentBlock;
        let thinking = super::content_block_json(&ContentBlock::Thinking {
            text: "ponder".to_owned(),
            signature: "sig".to_owned(),
        });
        assert_eq!(thinking["type"], "thinking");
        assert_eq!(thinking["thinking"], "ponder");
        assert_eq!(thinking["signature"], "sig");
        let redacted = super::content_block_json(&ContentBlock::RedactedThinking {
            data: "blob".to_owned(),
        });
        assert_eq!(redacted["type"], "redacted_thinking");
        assert_eq!(redacted["data"], "blob");
    }

    #[test]
    fn effort_model_uses_output_config() {
        use goat_provider::Effort;
        let cfg = super::thinking_config("claude-opus-4-8", Some(Effort::High));
        assert_eq!(cfg.thinking.unwrap()["type"], "adaptive");
        assert_eq!(cfg.output_config.unwrap()["effort"], "high");
    }

    #[test]
    fn budget_model_uses_budget_tokens() {
        use goat_provider::Effort;
        let cfg = super::thinking_config("claude-sonnet-4-5-20250929", Some(Effort::Medium));
        assert_eq!(cfg.thinking.as_ref().unwrap()["type"], "enabled");
        assert_eq!(cfg.thinking.unwrap()["budget_tokens"], 8192);
        assert!(cfg.max_tokens > 8192);
        assert!(cfg.output_config.is_none());
    }

    #[test]
    fn off_disables_thinking() {
        use goat_provider::Effort;
        let cfg = super::thinking_config("claude-sonnet-4-5-20250929", Some(Effort::Off));
        assert!(cfg.thinking.is_none());
        let none = super::thinking_config("claude-opus-4-8", None);
        assert!(none.thinking.is_none());
    }
}
