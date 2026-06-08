use serde::{Deserialize, Serialize};
use tokio::{sync::mpsc, task::JoinHandle};

pub use goat_auth::{TokenSet, now_secs};
pub use goat_protocol::{AuthMethod, Effort};

use std::fmt;

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
        content: String,
        is_error: bool,
    },
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    pub effort: Option<Effort>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub tools: bool,
    pub auth: AuthMethod,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        message: String,
    },
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
}
