use std::collections::HashMap;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use goat_provider::{
    AuthMethod, ContentBlock, MessageRole, ModelEvent, ModelInfo, ModelProvider, ModelRequest,
    ProviderCapabilities, ProviderId, ProviderMessage,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
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
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_mins(5))
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("reqwest client"),
        }
    }

    pub fn local(provider_id: &'static str, base_url: &'static str) -> Self {
        Self::new(
            ProviderId::from(provider_id),
            base_url,
            None,
            AuthMethod::None,
        )
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<serde_json::Value>,
    stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<serde_json::Value>,
}

fn role_label(role: MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
    }
}

fn to_chat_messages(messages: &[ProviderMessage]) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for message in messages {
        let has_tool_use = message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolUse { .. }));
        let has_tool_result = message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }));
        if has_tool_use {
            let tool_calls: Vec<serde_json::Value> = message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolUse { id, name, input } => Some(json!({
                        "id": id,
                        "type": "function",
                        "function": { "name": name, "arguments": input.to_string() },
                    })),
                    _ => None,
                })
                .collect();
            let text = message.text_content();
            out.push(json!({
                "role": "assistant",
                "content": if text.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(text) },
                "tool_calls": tool_calls,
            }));
        } else if has_tool_result {
            for block in &message.content {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } = block
                {
                    out.push(json!({
                        "role": "tool",
                        "tool_call_id": tool_use_id,
                        "content": content,
                    }));
                }
            }
        } else {
            out.push(json!({
                "role": role_label(message.role),
                "content": message.text_content(),
            }));
        }
    }
    out
}

fn to_chat_tools(req: &ModelRequest) -> Vec<serde_json::Value> {
    req.tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                },
            })
        })
        .collect()
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
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct ChatDelta {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallChunk>,
}

#[derive(Deserialize)]
struct ToolCallChunk {
    index: u32,
    id: Option<String>,
    function: Option<ToolCallFunction>,
}

#[derive(Deserialize)]
struct ToolCallFunction {
    name: Option<String>,
    arguments: Option<String>,
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

type ToolAccumulator = HashMap<u32, (String, String, String)>;

fn accumulate_tool_calls(tool_calls: &mut ToolAccumulator, deltas: Vec<ToolCallChunk>) {
    for call in deltas {
        let entry = tool_calls.entry(call.index).or_default();
        if let Some(id) = call.id {
            entry.0 = id;
        }
        if let Some(function) = call.function {
            if let Some(name) = function.name {
                entry.1 = name;
            }
            if let Some(arguments) = function.arguments {
                entry.2.push_str(&arguments);
            }
        }
    }
}

fn drain_tool_calls(tool_calls: &mut ToolAccumulator) -> Vec<ModelEvent> {
    let mut entries: Vec<(u32, (String, String, String))> = tool_calls.drain().collect();
    entries.sort_by_key(|(index, _)| *index);
    entries
        .into_iter()
        .map(|(_, (id, name, input))| ModelEvent::ToolCall { id, name, input })
        .collect()
}

async fn stream_chat(response: reqwest::Response, events: &mpsc::Sender<ModelEvent>) {
    let mut stream = response.bytes_stream().eventsource();
    let mut tool_calls: ToolAccumulator = HashMap::new();
    while let Some(event) = stream.next().await {
        match event {
            Ok(event) => {
                if event.data == "[DONE]" {
                    break;
                }
                let Ok(chunk) = serde_json::from_str::<ChatChunk>(&event.data) else {
                    continue;
                };
                let Some(choice) = chunk.choices.into_iter().next() else {
                    continue;
                };
                if let Some(text) = choice.delta.content
                    && events.send(ModelEvent::TextDelta { text }).await.is_err()
                {
                    return;
                }
                accumulate_tool_calls(&mut tool_calls, choice.delta.tool_calls);
                if choice.finish_reason.is_some() {
                    for call in drain_tool_calls(&mut tool_calls) {
                        if events.send(call).await.is_err() {
                            return;
                        }
                    }
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
    for call in drain_tool_calls(&mut tool_calls) {
        if events.send(call).await.is_err() {
            return;
        }
    }
    let _ = events.send(ModelEvent::Completed).await;
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

    fn validate(&self) -> JoinHandle<Result<(), String>> {
        let client = self.client.clone();
        let url = format!("{}/models", self.base_url);
        let bearer = self.bearer.clone();
        let auth = self.auth;
        tokio::spawn(async move {
            if matches!(auth, AuthMethod::None) {
                return Ok(());
            }
            let Some(token) = bearer else {
                return Err("no credentials".to_owned());
            };
            let resp = client
                .get(&url)
                .bearer_auth(token)
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

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            tools: true,
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
                messages: to_chat_messages(&req.messages),
                stream: true,
                tools: to_chat_tools(&req),
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
            stream_chat(resp, &events).await;
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
    use super::{
        ChatChunk, ToolAccumulator, accumulate_tool_calls, drain_tool_calls, to_chat_messages,
    };
    use goat_provider::{ContentBlock, MessageRole, ModelEvent, ProviderMessage};
    use serde_json::json;

    fn chunk_tool_calls(data: &str) -> Vec<super::ToolCallChunk> {
        let chunk: ChatChunk = serde_json::from_str(data).unwrap();
        chunk.choices.into_iter().next().unwrap().delta.tool_calls
    }

    #[test]
    fn plain_text_message_uses_text_role() {
        let messages = vec![ProviderMessage::text(MessageRole::User, "hi")];
        let out = to_chat_messages(&messages);
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["content"], "hi");
        assert!(out[0].get("tool_calls").is_none());
    }

    #[test]
    fn tool_use_becomes_assistant_tool_calls() {
        let messages = vec![ProviderMessage {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_1".to_owned(),
                name: "read_file".to_owned(),
                input: json!({ "path": "a.txt" }),
            }],
        }];
        let out = to_chat_messages(&messages);
        assert_eq!(out[0]["role"], "assistant");
        assert!(out[0]["content"].is_null());
        assert_eq!(out[0]["tool_calls"][0]["id"], "call_1");
        assert_eq!(out[0]["tool_calls"][0]["type"], "function");
        assert_eq!(out[0]["tool_calls"][0]["function"]["name"], "read_file");
        assert_eq!(
            out[0]["tool_calls"][0]["function"]["arguments"],
            r#"{"path":"a.txt"}"#
        );
    }

    #[test]
    fn tool_result_becomes_tool_role_message() {
        let messages = vec![ProviderMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_owned(),
                content: "file body".to_owned(),
                is_error: false,
            }],
        }];
        let out = to_chat_messages(&messages);
        assert_eq!(out[0]["role"], "tool");
        assert_eq!(out[0]["tool_call_id"], "call_1");
        assert_eq!(out[0]["content"], "file body");
    }

    #[test]
    fn accumulates_streamed_tool_call() {
        let mut tool_calls: ToolAccumulator = ToolAccumulator::new();
        let first = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{\"path\":"}}]}}]}"#;
        let second = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"a.txt\"}"}}]}}]}"#;
        accumulate_tool_calls(&mut tool_calls, chunk_tool_calls(first));
        accumulate_tool_calls(&mut tool_calls, chunk_tool_calls(second));
        let events = drain_tool_calls(&mut tool_calls);
        assert_eq!(
            events,
            vec![ModelEvent::ToolCall {
                id: "call_1".to_owned(),
                name: "read_file".to_owned(),
                input: r#"{"path":"a.txt"}"#.to_owned(),
            }]
        );
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn drains_multiple_tool_calls_in_index_order() {
        let mut tool_calls: ToolAccumulator = ToolAccumulator::new();
        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"b","function":{"name":"two","arguments":"{}"}},{"index":0,"id":"a","function":{"name":"one","arguments":"{}"}}]}}]}"#;
        accumulate_tool_calls(&mut tool_calls, chunk_tool_calls(data));
        let events = drain_tool_calls(&mut tool_calls);
        assert_eq!(
            events,
            vec![
                ModelEvent::ToolCall {
                    id: "a".to_owned(),
                    name: "one".to_owned(),
                    input: "{}".to_owned(),
                },
                ModelEvent::ToolCall {
                    id: "b".to_owned(),
                    name: "two".to_owned(),
                    input: "{}".to_owned(),
                },
            ]
        );
    }
}
