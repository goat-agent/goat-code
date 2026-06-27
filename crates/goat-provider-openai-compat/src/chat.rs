use std::collections::HashMap;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use goat_provider::{
    AuthMethod, Capabilities, ContentBlock, Effort, Message, MessageRole, Model, Provider,
    ProviderId, Request, StreamEvent, Usage,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::common;

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
            client: common::http_client(),
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
    stream_options: StreamOptions,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'a str>,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

fn chat_effort_wire(effort: Effort) -> Option<&'static str> {
    match effort {
        Effort::Off => None,
        Effort::Low => Some("low"),
        Effort::Medium => Some("medium"),
        Effort::High | Effort::Xhigh | Effort::Max => Some("high"),
    }
}

fn role_label(role: MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
    }
}

fn text_and_images_content(message: &Message) -> serde_json::Value {
    let images: Vec<(&String, &String)> = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Image { media_type, data } => Some((media_type, data)),
            _ => None,
        })
        .collect();
    let text = message.text_content();
    if images.is_empty() {
        return serde_json::Value::String(text);
    }
    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(json!({ "type": "text", "text": text }));
    }
    for (media_type, data) in images {
        content.push(json!({
            "type": "image_url",
            "image_url": { "url": format!("data:{media_type};base64,{data}") },
        }));
    }
    serde_json::Value::Array(content)
}

fn to_chat_messages(messages: &[Message]) -> Vec<serde_json::Value> {
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
                        "function": { "name": name, "arguments": common::tool_arguments(input) },
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
                    let output_text = ContentBlock::tool_result_text(content);
                    out.push(json!({
                        "role": "tool",
                        "tool_call_id": tool_use_id,
                        "content": output_text,
                    }));
                }
            }
        } else {
            out.push(json!({
                "role": role_label(message.role),
                "content": text_and_images_content(message),
            }));
        }
    }
    out
}

fn to_chat_tools(req: &Request) -> Vec<serde_json::Value> {
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
    usage: Option<ChatUsage>,
}

#[derive(Deserialize)]
struct ChatUsage {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
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

fn drain_tool_calls(tool_calls: &mut ToolAccumulator) -> Vec<StreamEvent> {
    let mut entries: Vec<(u32, (String, String, String))> = tool_calls.drain().collect();
    entries.sort_by_key(|(index, _)| *index);
    entries
        .into_iter()
        .map(|(_, (id, name, input))| StreamEvent::ToolCall { id, name, input })
        .collect()
}

fn data_has_error(data: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(data)
        .ok()
        .and_then(|value| value.get("error").map(|_| ()))
        .is_some()
}

async fn stream_chat(response: reqwest::Response, events: &mpsc::Sender<StreamEvent>) {
    let mut stream = response.bytes_stream().eventsource();
    let mut tool_calls: ToolAccumulator = HashMap::new();
    let mut last_usage: Option<ChatUsage> = None;
    while let Some(event) = stream.next().await {
        match event {
            Ok(event) => {
                if event.data == "[DONE]" {
                    break;
                }
                if event.event == "error" || data_has_error(&event.data) {
                    let _ = events
                        .send(StreamEvent::Failed {
                            error: common::classify_stream_error(&event.data),
                        })
                        .await;
                    return;
                }
                let Ok(chunk) = serde_json::from_str::<ChatChunk>(&event.data) else {
                    continue;
                };
                if chunk.usage.is_some() {
                    last_usage = chunk.usage;
                }
                let Some(choice) = chunk.choices.into_iter().next() else {
                    continue;
                };
                if let Some(text) = choice.delta.content
                    && events.send(StreamEvent::TextDelta { text }).await.is_err()
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
                    .send(StreamEvent::Failed {
                        error: goat_provider::StreamError::transport(err.to_string()),
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
    if let Some(u) = last_usage {
        let usage = Usage {
            input_tokens: u.prompt_tokens.unwrap_or(0),
            output_tokens: u.completion_tokens.unwrap_or(0),
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        let _ = events.send(StreamEvent::Usage { usage }).await;
    }
    let _ = events.send(StreamEvent::Completed).await;
}

impl Provider for OpenAiCompatProvider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    fn authenticated(&self) -> bool {
        common::authenticated(self.auth, &self.bearer)
    }

    fn validate(&self) -> JoinHandle<Result<(), String>> {
        common::validate_bearer(
            self.client.clone(),
            format!("{}/models", self.base_url),
            self.auth,
            self.bearer.clone(),
        )
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tools: true,
            auth: self.auth,
            images: true,
        }
    }

    fn supports_images(&self, model: &str) -> bool {
        crate::vision::known_openai_compatible_vision_model(model)
    }

    fn efforts(&self, _model: &str) -> Vec<Effort> {
        vec![Effort::Low, Effort::Medium, Effort::High]
    }

    fn stream(&self, req: Request, events: mpsc::Sender<StreamEvent>) -> JoinHandle<()> {
        let client = self.client.clone();
        let url = format!("{}/chat/completions", self.base_url);
        let bearer = self.bearer.clone();
        tokio::spawn(async move {
            let body = ChatRequest {
                model: &req.model,
                messages: to_chat_messages(&req.messages),
                stream: true,
                stream_options: StreamOptions {
                    include_usage: true,
                },
                tool_choice: (!req.tools.is_empty()
                    && matches!(req.tool_choice, goat_provider::ToolChoice::None))
                .then_some("none"),
                tools: to_chat_tools(&req),
                reasoning_effort: req.effort.and_then(chat_effort_wire),
            };
            let mut builder = client.post(&url).json(&body);
            if let Some(token) = &bearer {
                builder = builder.bearer_auth(token);
            }
            let resp = match builder.send().await {
                Ok(resp) => resp,
                Err(err) => {
                    let _ = events
                        .send(StreamEvent::Failed {
                            error: common::transport(&err),
                        })
                        .await;
                    return;
                }
            };
            if !resp.status().is_success() {
                let status = resp.status();
                let headers = resp.headers().clone();
                let detail = resp.text().await.unwrap_or_default();
                let _ = events
                    .send(StreamEvent::Failed {
                        error: common::classify_http(status, &headers, &detail),
                    })
                    .await;
                return;
            }
            stream_chat(resp, &events).await;
        })
    }

    fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
        common::discover_models(
            self.client.clone(),
            format!("{}/models", self.base_url),
            self.bearer.clone(),
            None,
            crate::vision::known_openai_compatible_vision_model,
            out,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChatChunk, ToolAccumulator, accumulate_tool_calls, data_has_error, drain_tool_calls,
        to_chat_messages,
    };
    use goat_provider::{ContentBlock, Message, MessageRole, StreamEvent};
    use serde_json::json;

    fn chunk_tool_calls(data: &str) -> Vec<super::ToolCallChunk> {
        let chunk: ChatChunk = serde_json::from_str(data).unwrap();
        chunk.choices.into_iter().next().unwrap().delta.tool_calls
    }

    #[test]
    fn error_chunk_is_detected() {
        assert!(data_has_error(
            r#"{"error":{"message":"bad","type":"invalid_request_error"}}"#
        ));
        assert!(!data_has_error(
            r#"{"choices":[{"delta":{"content":"hi"}}]}"#
        ));
        assert!(!data_has_error("not json"));
    }

    #[test]
    fn plain_text_message_uses_text_role() {
        let messages = vec![Message::text(MessageRole::User, "hi")];
        let out = to_chat_messages(&messages);
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["content"], "hi");
        assert!(out[0].get("tool_calls").is_none());
    }

    #[test]
    fn tool_use_becomes_assistant_tool_calls() {
        let messages = vec![Message {
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
        let messages = vec![Message {
            role: MessageRole::User,
            content: vec![ContentBlock::text_result("call_1", "file body", false)],
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
            vec![StreamEvent::ToolCall {
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
                StreamEvent::ToolCall {
                    id: "a".to_owned(),
                    name: "one".to_owned(),
                    input: "{}".to_owned(),
                },
                StreamEvent::ToolCall {
                    id: "b".to_owned(),
                    name: "two".to_owned(),
                    input: "{}".to_owned(),
                },
            ]
        );
    }
}
