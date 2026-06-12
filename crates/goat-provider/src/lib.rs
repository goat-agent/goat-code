use serde::{Deserialize, Deserializer, Serialize};
use tokio::{sync::mpsc, task::JoinHandle};

pub use goat_auth::{TokenSet, now_secs};
pub use goat_protocol::{AuthMethod, Effort, RateLimitSnapshot, RateWindow, Usage};

use std::fmt;

fn deser_tool_result_content<'de, D>(d: D) -> Result<Vec<ContentBlock>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error as _;
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::String(s) => Ok(vec![ContentBlock::Text { text: s }]),
        serde_json::Value::Array(arr) => arr
            .into_iter()
            .map(|item| serde_json::from_value(item).map_err(D::Error::custom))
            .collect(),
        other => Err(D::Error::custom(format!(
            "expected string or array for tool_result content, got {other}"
        ))),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderId(pub String);

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ProviderId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        text: String,
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(deserialize_with = "deser_tool_result_content")]
        content: Vec<ContentBlock>,
        is_error: bool,
    },
    Image {
        media_type: String,
        data: String,
    },
}

impl ContentBlock {
    pub fn text_result(
        tool_use_id: impl Into<String>,
        text: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: vec![Self::Text { text: text.into() }],
            is_error,
        }
    }

    pub fn tool_result_text(content: &[ContentBlock]) -> String {
        content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn text(role: MessageRole, text: impl Into<String>) -> Self {
        Self {
            role,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    #[default]
    Auto,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    pub effort: Option<Effort>,
    #[serde(default)]
    pub tool_choice: ToolChoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub tools: bool,
    pub auth: AuthMethod,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StreamEvent {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ThinkingSignature {
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
    ToolCall {
        id: String,
        name: String,
        input: String,
    },
    Completed,
    Failed {
        error: StreamError,
    },
    Usage {
        usage: Usage,
    },
    RateLimits {
        snapshot: RateLimitSnapshot,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
pub enum StreamError {
    #[error("rate limited: {message}")]
    RateLimited {
        retry_after: Option<std::time::Duration>,
        message: String,
    },
    #[error("provider overloaded: {message}")]
    Overloaded { message: String },
    #[error("context window exceeded: {message}")]
    ContextOverflow { message: String },
    #[error("authentication failed: {message}")]
    Auth { message: String },
    #[error("invalid request: {message}")]
    InvalidRequest { message: String },
    #[error("connection failed: {message}")]
    Transport { message: String },
    #[error("{message}")]
    Other { message: String },
}

impl StreamError {
    pub fn rate_limited(
        message: impl Into<String>,
        retry_after: Option<std::time::Duration>,
    ) -> Self {
        Self::RateLimited {
            retry_after,
            message: message.into(),
        }
    }

    pub fn overloaded(message: impl Into<String>) -> Self {
        Self::Overloaded {
            message: message.into(),
        }
    }

    pub fn context_overflow(message: impl Into<String>) -> Self {
        Self::ContextOverflow {
            message: message.into(),
        }
    }

    pub fn auth(message: impl Into<String>) -> Self {
        Self::Auth {
            message: message.into(),
        }
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::InvalidRequest {
            message: message.into(),
        }
    }

    pub fn transport(message: impl Into<String>) -> Self {
        Self::Transport {
            message: message.into(),
        }
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self::Other {
            message: message.into(),
        }
    }
}

pub trait Provider: Send + Sync + 'static {
    fn id(&self) -> ProviderId;
    fn capabilities(&self) -> Capabilities;
    fn stream(&self, req: Request, tx: mpsc::Sender<StreamEvent>) -> JoinHandle<()>;
    fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()>;
    fn catalog(&self) -> &'static [&'static str] {
        &[]
    }
    fn efforts(&self, _model: &str) -> Vec<Effort> {
        Vec::new()
    }
    fn authenticated(&self) -> bool {
        true
    }
    fn validate(&self) -> JoinHandle<Result<(), String>> {
        tokio::spawn(async { Ok(()) })
    }
    fn context_window(&self, _model: &str) -> Option<u32> {
        None
    }

    fn supports_web_search(&self) -> bool {
        false
    }

    fn web_search(&self, query: String) -> JoinHandle<Result<Vec<SearchResult>, StreamError>> {
        let _ = query;
        tokio::spawn(async { Err(StreamError::other("web search is not supported")) })
    }

    fn login(&self, status: mpsc::Sender<String>) -> JoinHandle<Result<TokenSet, String>> {
        let _ = status;
        tokio::spawn(async { Err("login not supported".into()) })
    }
}

#[cfg(test)]
mod tests {
    use tokio::{sync::mpsc, task::JoinHandle};

    use super::{
        AuthMethod, Capabilities, Message, MessageRole, Model, Provider, ProviderId, Request,
        StreamEvent,
    };

    struct MockProvider;

    impl Provider for MockProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("mock")
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                tools: false,
                auth: AuthMethod::None,
            }
        }

        fn stream(&self, _req: Request, tx: mpsc::Sender<StreamEvent>) -> JoinHandle<()> {
            tokio::spawn(async move {
                let _ = tx.send(StreamEvent::TextDelta { text: "hi".into() }).await;
                let _ = tx.send(StreamEvent::Completed).await;
            })
        }

        fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
            tokio::spawn(async move {
                let _ = out
                    .send(Model {
                        id: "mock-1".into(),
                    })
                    .await;
            })
        }
    }

    #[tokio::test]
    async fn mock_provider_streams_events() {
        let provider = MockProvider;
        assert_eq!(provider.id(), ProviderId::from("mock"));
        assert!(!provider.capabilities().tools);
        let (tx, mut rx) = mpsc::channel(8);
        let handle = provider.stream(
            Request {
                model: "mock-1".into(),
                messages: vec![Message::text(MessageRole::User, "hi")],
                tools: vec![],
                effort: None,
                tool_choice: super::ToolChoice::Auto,
            },
            tx,
        );
        let mut events = Vec::new();
        while let Some(ev) = rx.recv().await {
            events.push(ev);
        }
        handle.await.unwrap();
        assert_eq!(
            events,
            vec![
                StreamEvent::TextDelta { text: "hi".into() },
                StreamEvent::Completed
            ]
        );
    }

    #[tokio::test]
    async fn mock_provider_discovers_models() {
        let provider = MockProvider;
        let (tx, mut rx) = mpsc::channel(8);
        let handle = provider.discover(tx);
        let info = rx.recv().await.unwrap();
        assert_eq!(info.id, "mock-1");
        handle.await.unwrap();
    }

    #[test]
    fn content_blocks_round_trip_through_json() {
        use super::ContentBlock;
        let blocks = vec![
            ContentBlock::Text {
                text: "hello".into(),
            },
            ContentBlock::Thinking {
                text: "step one".into(),
                signature: "sig-abc".into(),
            },
            ContentBlock::RedactedThinking {
                data: "opaque".into(),
            },
            ContentBlock::ToolUse {
                id: "call-1".into(),
                name: "Read".into(),
                input: serde_json::json!({"path": "src/lib.rs"}),
            },
            ContentBlock::ToolResult {
                tool_use_id: "call-1".into(),
                content: vec![ContentBlock::Text {
                    text: "result".into(),
                }],
                is_error: false,
            },
            ContentBlock::Image {
                media_type: "image/png".into(),
                data: "base64data".into(),
            },
        ];
        let json = serde_json::to_string(&blocks).unwrap();
        let restored: Vec<ContentBlock> = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, blocks);
    }
}
