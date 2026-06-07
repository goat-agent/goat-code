use std::{
    fmt,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tokio::{sync::mpsc, task::JoinHandle};

pub use goat_protocol::AuthMethod;

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or(0)
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
pub struct ProviderMessage {
    pub role: MessageRole,
    pub content: Vec<ContentBlock>,
}

impl ProviderMessage {
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
pub struct ModelInfo {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    pub model: String,
    pub messages: Vec<ProviderMessage>,
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub tools: bool,
    pub auth: AuthMethod,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelEvent {
    TextDelta {
        text: String,
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

pub trait ModelProvider: Send + Sync + 'static {
    fn id(&self) -> ProviderId;
    fn capabilities(&self) -> ProviderCapabilities;
    fn request(&self, req: ModelRequest, events: mpsc::Sender<ModelEvent>) -> JoinHandle<()>;
    fn discover(&self, out: mpsc::Sender<ModelInfo>) -> JoinHandle<()>;
    fn catalog(&self) -> &'static [&'static str] {
        &[]
    }
    fn authenticated(&self) -> bool {
        true
    }
    fn validate(&self) -> JoinHandle<Result<(), String>> {
        tokio::spawn(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use tokio::{sync::mpsc, task::JoinHandle};

    use super::{
        AuthMethod, MessageRole, ModelEvent, ModelInfo, ModelProvider, ModelRequest,
        ProviderCapabilities, ProviderId, ProviderMessage,
    };

    struct MockProvider;

    impl ModelProvider for MockProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("mock")
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                tools: false,
                auth: AuthMethod::None,
            }
        }

        fn request(&self, _req: ModelRequest, events: mpsc::Sender<ModelEvent>) -> JoinHandle<()> {
            tokio::spawn(async move {
                let _ = events
                    .send(ModelEvent::TextDelta { text: "hi".into() })
                    .await;
                let _ = events.send(ModelEvent::Completed).await;
            })
        }

        fn discover(&self, out: mpsc::Sender<ModelInfo>) -> JoinHandle<()> {
            tokio::spawn(async move {
                let _ = out
                    .send(ModelInfo {
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
        let handle = provider.request(
            ModelRequest {
                model: "mock-1".into(),
                messages: vec![ProviderMessage::text(MessageRole::User, "hi")],
                tools: vec![],
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
                ModelEvent::TextDelta { text: "hi".into() },
                ModelEvent::Completed
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
