use std::collections::HashMap;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use goat_provider::{
    AuthMethod, ContentBlock, MessageRole, ModelEvent, ModelInfo, ModelProvider, ModelRequest,
    ProviderCapabilities, ProviderId, ProviderMessage, ToolDefinition,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{sync::mpsc, task::JoinHandle};

#[derive(Serialize)]
struct ResponsesRequest<'a> {
    model: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'a str>,
    input: Vec<serde_json::Value>,
    tools: Vec<serde_json::Value>,
    tool_choice: &'a str,
    parallel_tool_calls: bool,
    store: bool,
    stream: bool,
}

fn text_item(role: &str, content_kind: &str, text: &str) -> serde_json::Value {
    json!({
        "type": "message",
        "role": role,
        "content": [{ "type": content_kind, "text": text }],
    })
}

fn append_message_items(
    message: &ProviderMessage,
    role: &str,
    content_kind: &str,
    input: &mut Vec<serde_json::Value>,
) {
    let mut text = String::new();
    for block in &message.content {
        match block {
            ContentBlock::Text { text: chunk } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(chunk);
            }
            ContentBlock::ToolUse {
                id,
                name,
                input: args,
            } => {
                input.push(json!({
                    "type": "function_call",
                    "call_id": id,
                    "name": name,
                    "arguments": args.to_string(),
                }));
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => {
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": tool_use_id,
                    "output": content,
                }));
            }
        }
    }
    if !text.is_empty() {
        input.push(text_item(role, content_kind, &text));
    }
}

pub fn build_body(
    model: &str,
    messages: &[ProviderMessage],
    tools: &[ToolDefinition],
    default_instructions: Option<&str>,
    store: bool,
) -> serde_json::Value {
    let mut instructions = String::new();
    let mut input: Vec<serde_json::Value> = Vec::new();
    for message in messages {
        match message.role {
            MessageRole::System => {
                if !instructions.is_empty() {
                    instructions.push('\n');
                }
                instructions.push_str(&message.text_content());
            }
            MessageRole::User => append_message_items(message, "user", "input_text", &mut input),
            MessageRole::Assistant => {
                append_message_items(message, "assistant", "output_text", &mut input);
            }
        }
    }
    let instructions = if instructions.is_empty() {
        default_instructions
    } else {
        Some(instructions.as_str())
    };
    let tools = tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.input_schema,
            })
        })
        .collect();
    let request = ResponsesRequest {
        model,
        instructions,
        input,
        tools,
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
    let mut tool_calls: HashMap<String, (String, String, String)> = HashMap::new();
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
                "response.output_item.added" => {
                    if let Some(call) = parse_function_call_item(&event.data) {
                        tool_calls.insert(call.item_id, (call.call_id, call.name, String::new()));
                    }
                }
                "response.function_call_arguments.delta" => {
                    if let Some(delta) = parse_arguments_delta(&event.data)
                        && let Some(entry) = tool_calls.get_mut(&delta.item_id)
                    {
                        entry.2.push_str(&delta.delta);
                    }
                }
                "response.output_item.done" | "response.function_call_arguments.done" => {
                    if let Some(item_id) = parse_item_id(&event.data)
                        && let Some((call_id, name, input)) = tool_calls.remove(&item_id)
                        && events
                            .send(ModelEvent::ToolCall {
                                id: call_id,
                                name,
                                input,
                            })
                            .await
                            .is_err()
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
struct OutputItemAdded {
    item: OutputItem,
}

#[derive(Deserialize)]
struct OutputItem {
    #[serde(rename = "type")]
    kind: String,
    id: Option<String>,
    call_id: Option<String>,
    name: Option<String>,
}

struct FunctionCallItem {
    item_id: String,
    call_id: String,
    name: String,
}

fn parse_function_call_item(data: &str) -> Option<FunctionCallItem> {
    let item = serde_json::from_str::<OutputItemAdded>(data).ok()?.item;
    if item.kind != "function_call" {
        return None;
    }
    Some(FunctionCallItem {
        item_id: item.id?,
        call_id: item.call_id?,
        name: item.name?,
    })
}

#[derive(Deserialize)]
struct ArgumentsDelta {
    item_id: String,
    delta: String,
}

fn parse_arguments_delta(data: &str) -> Option<ArgumentsDelta> {
    serde_json::from_str::<ArgumentsDelta>(data).ok()
}

#[derive(Deserialize)]
struct ItemRef {
    item_id: Option<String>,
    item: Option<OutputItem>,
}

fn parse_item_id(data: &str) -> Option<String> {
    let parsed = serde_json::from_str::<ItemRef>(data).ok()?;
    parsed
        .item_id
        .or_else(|| parsed.item.and_then(|item| item.id))
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
            tools: true,
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
            let body = build_body(&req.model, &req.messages, &req.tools, None, false);
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
    use super::{
        build_body, parse_arguments_delta, parse_function_call_item, parse_item_id,
        parse_output_delta,
    };
    use goat_provider::{ContentBlock, MessageRole, ProviderMessage, ToolDefinition};
    use serde_json::json;

    #[test]
    fn parses_output_text_delta() {
        let data = r#"{"type":"response.output_text.delta","delta":"Hi"}"#;
        assert_eq!(parse_output_delta(data).as_deref(), Some("Hi"));
    }

    #[test]
    fn default_instructions_used_when_no_system_message() {
        let messages = vec![ProviderMessage::text(MessageRole::User, "hi")];
        let body = build_body("gpt-5.5", &messages, &[], Some("base"), false);
        assert_eq!(body["instructions"], "base");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
    }

    #[test]
    fn system_message_overrides_default_instructions() {
        let messages = vec![
            ProviderMessage::text(MessageRole::System, "be terse"),
            ProviderMessage::text(MessageRole::User, "hi"),
        ];
        let body = build_body("gpt-5.5", &messages, &[], Some("base"), false);
        assert_eq!(body["instructions"], "be terse");
    }

    #[test]
    fn instructions_omitted_when_empty_and_no_default() {
        let messages = vec![ProviderMessage::text(MessageRole::User, "hi")];
        let body = build_body("gpt-5.5", &messages, &[], None, false);
        assert!(body.get("instructions").is_none());
    }

    #[test]
    fn serializes_tool_definitions() {
        let tools = vec![ToolDefinition {
            name: "read_file".to_owned(),
            description: "reads a file".to_owned(),
            input_schema: json!({ "type": "object" }),
        }];
        let messages = vec![ProviderMessage::text(MessageRole::User, "hi")];
        let body = build_body("gpt-5.5", &messages, &tools, None, false);
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "read_file");
        assert_eq!(body["tools"][0]["parameters"]["type"], "object");
    }

    #[test]
    fn serializes_tool_use_and_result_items() {
        let assistant = ProviderMessage {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_1".to_owned(),
                name: "read_file".to_owned(),
                input: json!({ "path": "a.txt" }),
            }],
        };
        let result = ProviderMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_owned(),
                content: "file body".to_owned(),
                is_error: false,
            }],
        };
        let body = build_body("gpt-5.5", &[assistant, result], &[], None, false);
        assert_eq!(body["input"][0]["type"], "function_call");
        assert_eq!(body["input"][0]["call_id"], "call_1");
        assert_eq!(body["input"][0]["name"], "read_file");
        assert_eq!(body["input"][0]["arguments"], r#"{"path":"a.txt"}"#);
        assert_eq!(body["input"][1]["type"], "function_call_output");
        assert_eq!(body["input"][1]["call_id"], "call_1");
        assert_eq!(body["input"][1]["output"], "file body");
    }

    #[test]
    fn accumulates_function_call_from_stream() {
        let added = r#"{"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read_file"}}"#;
        let call = parse_function_call_item(added).unwrap();
        assert_eq!(call.item_id, "fc_1");
        assert_eq!(call.call_id, "call_1");
        assert_eq!(call.name, "read_file");

        let first = r#"{"item_id":"fc_1","delta":"{\"path\":"}"#;
        let second = r#"{"item_id":"fc_1","delta":"\"a.txt\"}"}"#;
        let mut buf = String::new();
        for chunk in [first, second] {
            let delta = parse_arguments_delta(chunk).unwrap();
            assert_eq!(delta.item_id, "fc_1");
            buf.push_str(&delta.delta);
        }
        assert_eq!(buf, r#"{"path":"a.txt"}"#);

        let done = r#"{"item_id":"fc_1"}"#;
        assert_eq!(parse_item_id(done).as_deref(), Some("fc_1"));
        let done_item = r#"{"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read_file"}}"#;
        assert_eq!(parse_item_id(done_item).as_deref(), Some("fc_1"));
    }
}
