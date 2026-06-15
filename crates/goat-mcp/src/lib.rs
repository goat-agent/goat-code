use std::{
    collections::{HashMap, HashSet},
    path::Path,
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use goat_protocol::ToolDisplay;
use goat_tool::{Tool, ToolContext, ToolError, ToolFuture, ToolImage, ToolOutput};
use rmcp::{
    RoleClient, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResult, ClientRequest, Content, RawContent,
        ResourceContents, ServerResult, Tool as McpTool,
    },
    service::{PeerRequestOptions, RunningService, ServiceError},
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;

const START_TIMEOUT: Duration = Duration::from_secs(10);
const CALL_TIMEOUT: Duration = Duration::from_mins(2);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("mcp config io failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("mcp config json failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("mcp server {server} failed to initialize: {message}")]
    Initialize { server: String, message: String },
    #[error("mcp server {server} request failed: {source}")]
    Request {
        server: String,
        #[source]
        source: ServiceError,
    },
    #[error("mcp tool {tool} returned an error: {message}")]
    ToolError { tool: String, message: String },
    #[error("mcp tool input must be a json object")]
    InputNotObject,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpConfig {
    #[serde(default)]
    pub mcp_servers: HashMap<String, ServerConfig>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

impl McpConfig {
    pub fn load(path: &Path) -> Result<Self, McpError> {
        let raw = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }
}

pub async fn load_manager(path: Option<&Path>) -> Arc<McpManager> {
    let Some(path) = path else {
        return Arc::new(McpManager::default());
    };
    if !path.exists() {
        return Arc::new(McpManager::default());
    }
    match McpConfig::load(path) {
        Ok(config) => McpManager::start(config).await,
        Err(err) => {
            tracing::warn!(%err, path = %path.display(), "failed to load mcp config");
            Arc::new(McpManager::default())
        }
    }
}

#[derive(Default)]
pub struct McpManager {
    tools: Vec<McpToolAdapter>,
    sessions: Vec<Arc<McpSession>>,
}

impl McpManager {
    pub async fn start(config: McpConfig) -> Arc<Self> {
        let mut tools = Vec::new();
        let mut sessions = Vec::new();
        let mut used_names = HashSet::new();
        let mut servers: Vec<_> = config.mcp_servers.into_iter().collect();
        servers.sort_by(|a, b| a.0.cmp(&b.0));
        for (server_name, server_config) in servers {
            match McpSession::start(server_name.clone(), server_config).await {
                Ok((session, discovered)) => {
                    let session = Arc::new(session);
                    for tool in discovered {
                        let exposed_name =
                            unique_tool_name(&mut used_names, &server_name, &tool.name);
                        tools.push(McpToolAdapter::new(
                            exposed_name,
                            server_name.clone(),
                            tool,
                            session.clone(),
                        ));
                    }
                    sessions.push(session);
                }
                Err(err) => tracing::warn!(%err, server = %server_name, "skipping mcp server"),
            }
        }
        tools.sort_by(|a, b| a.name.cmp(b.name));
        Arc::new(Self { tools, sessions })
    }

    pub fn tools(&self) -> Vec<Box<dyn Tool>> {
        self.tools
            .iter()
            .cloned()
            .map(|tool| Box::new(tool) as Box<dyn Tool>)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub async fn shutdown(&self) {
        for session in &self.sessions {
            session.shutdown().await;
        }
    }
}

struct McpSession {
    server_name: String,
    client: Mutex<RunningService<RoleClient, ()>>,
}

impl McpSession {
    async fn start(
        server_name: String,
        config: ServerConfig,
    ) -> Result<(Self, Vec<McpTool>), McpError> {
        let mut command = Command::new(&config.command);
        command.args(&config.args).envs(&config.env);
        let (transport, stderr) = TokioChildProcess::builder(command.configure(|cmd| {
            cmd.stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
        }))
        .spawn()?;
        if let Some(stderr) = stderr {
            spawn_stderr_logger(server_name.clone(), stderr);
        }
        let token = CancellationToken::new();
        let client = tokio::time::timeout(START_TIMEOUT, ().serve_with_ct(transport, token))
            .await
            .map_err(|_| McpError::Initialize {
                server: server_name.clone(),
                message: format!("timed out after {}s", START_TIMEOUT.as_secs()),
            })?
            .map_err(|err| McpError::Initialize {
                server: server_name.clone(),
                message: err.to_string(),
            })?;
        let tools = list_all_tools_with_timeout(&client, &server_name).await?;
        Ok((
            Self {
                server_name,
                client: Mutex::new(client),
            },
            tools,
        ))
    }

    async fn call(&self, tool_name: &str, input: &str) -> Result<ToolOutput, McpError> {
        let args = input_arguments(input)?;
        let params = CallToolRequestParams::new(tool_name.to_owned()).with_arguments(args);
        let request = ClientRequest::CallToolRequest(rmcp::model::CallToolRequest::new(params));
        let client = self.client.lock().await;
        let handle = client
            .send_cancellable_request(request, request_options(CALL_TIMEOUT))
            .await
            .map_err(|source| McpError::Request {
                server: self.server_name.clone(),
                source,
            })?;
        let result = handle
            .await_response()
            .await
            .map_err(|source| McpError::Request {
                server: self.server_name.clone(),
                source,
            })?;
        let ServerResult::CallToolResult(result) = result else {
            return Err(McpError::Request {
                server: self.server_name.clone(),
                source: ServiceError::UnexpectedResponse,
            });
        };
        convert_result(tool_name, result)
    }

    async fn shutdown(&self) {
        let mut client = self.client.lock().await;
        if let Err(err) = client.close_with_timeout(SHUTDOWN_TIMEOUT).await {
            tracing::warn!(%err, server = %self.server_name, "failed to close mcp session");
        }
    }
}

#[derive(Clone)]
pub struct McpToolAdapter {
    name: &'static str,
    description: &'static str,
    parameters: Value,
    original_name: String,
    server_name: String,
    session: Arc<McpSession>,
}

impl McpToolAdapter {
    fn new(
        exposed_name: String,
        server_name: String,
        tool: McpTool,
        session: Arc<McpSession>,
    ) -> Self {
        let description = tool
            .description
            .map_or_else(String::new, std::borrow::Cow::into_owned);
        Self {
            name: leak(exposed_name),
            description: leak(description),
            parameters: Value::Object((*tool.input_schema).clone()),
            original_name: tool.name.into_owned(),
            server_name,
            session,
        }
    }
}

impl Tool for McpToolAdapter {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        self.description
    }

    fn parameters(&self) -> Value {
        self.parameters.clone()
    }

    fn run<'a>(&'a self, input: &'a str, _ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            self.session
                .call(&self.original_name, input)
                .await
                .map_err(|err| ToolError::Execution {
                    message: err.to_string(),
                })
        })
    }

    fn display_input(&self, input: &str) -> ToolDisplay {
        ToolDisplay::with_detail(
            format!("{} on {}", self.original_name, self.server_name),
            input.to_owned(),
        )
    }
}

async fn list_all_tools_with_timeout(
    client: &RunningService<RoleClient, ()>,
    server_name: &str,
) -> Result<Vec<McpTool>, McpError> {
    tokio::time::timeout(CALL_TIMEOUT, client.list_all_tools())
        .await
        .map_err(|_| McpError::Initialize {
            server: server_name.to_owned(),
            message: format!("tools/list timed out after {}s", CALL_TIMEOUT.as_secs()),
        })?
        .map_err(|source| McpError::Request {
            server: server_name.to_owned(),
            source,
        })
}

fn request_options(timeout: Duration) -> PeerRequestOptions {
    let mut options = PeerRequestOptions::no_options();
    options.timeout = Some(timeout);
    options
}

fn spawn_stderr_logger(server_name: String, stderr: tokio::process::ChildStderr) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    tracing::warn!(server = %server_name, stream = "stderr", "{line}");
                }
                Ok(None) => break,
                Err(err) => {
                    tracing::warn!(%err, server = %server_name, "failed to read mcp stderr");
                    break;
                }
            }
        }
    });
}

fn input_arguments(input: &str) -> Result<Map<String, Value>, McpError> {
    match serde_json::from_str::<Value>(input)? {
        Value::Object(map) => Ok(map),
        _ => Err(McpError::InputNotObject),
    }
}

fn convert_result(tool_name: &str, result: CallToolResult) -> Result<ToolOutput, McpError> {
    let mut fallback = Vec::new();
    let mut first_image = None;
    for content in result.content {
        match content_to_tool_content(content) {
            ToolResultPart::Text(text) => fallback.push(text),
            ToolResultPart::Image(image) => {
                if first_image.is_none() {
                    first_image = Some(image);
                }
            }
        }
    }
    if let Some(value) = result.structured_content {
        fallback.push(format!("structuredContent: {value}"));
    }
    if result.is_error.unwrap_or(false) {
        let message = if fallback.is_empty() {
            "MCP tool returned an error".to_owned()
        } else {
            fallback.join("\n")
        };
        return Err(McpError::ToolError {
            tool: tool_name.to_owned(),
            message,
        });
    }
    if !fallback.is_empty() {
        Ok(ToolOutput::text(fallback.join("\n")).with_summary(summary(&fallback)))
    } else if let Some(image) = first_image {
        Ok(ToolOutput::image(image).with_summary("image"))
    } else {
        Ok(ToolOutput::text(String::new()))
    }
}

enum ToolResultPart {
    Text(String),
    Image(ToolImage),
}

fn content_to_tool_content(content: Content) -> ToolResultPart {
    match content.raw {
        RawContent::Text(text) => ToolResultPart::Text(text.text),
        RawContent::Image(image) => ToolResultPart::Image(ToolImage {
            media_type: image.mime_type,
            data: image.data,
        }),
        RawContent::Audio(audio) => ToolResultPart::Text(format!(
            "audio result: mimeType={}, base64Bytes={}",
            audio.mime_type,
            audio.data.len()
        )),
        RawContent::Resource(resource) => {
            ToolResultPart::Text(resource_fallback(resource.resource))
        }
        RawContent::ResourceLink(resource) => ToolResultPart::Text(format!(
            "resource link: uri={}, name={}",
            resource.uri, resource.name
        )),
    }
}

fn resource_fallback(resource: ResourceContents) -> String {
    match resource {
        ResourceContents::TextResourceContents {
            uri,
            mime_type,
            text,
            ..
        } => format!(
            "embedded resource: uri={}, mimeType={}\n{}",
            uri,
            mime_type.unwrap_or_default(),
            text
        ),
        ResourceContents::BlobResourceContents {
            uri,
            mime_type,
            blob,
            ..
        } => format!(
            "embedded resource: uri={}, mimeType={}, base64Bytes={}",
            uri,
            mime_type.unwrap_or_default(),
            blob.len()
        ),
    }
}

fn summary(parts: &[String]) -> String {
    parts
        .iter()
        .find_map(|part| part.lines().find(|line| !line.trim().is_empty()))
        .map_or_else(String::new, |line| line.chars().take(80).collect())
}

pub fn sanitize_component(input: &str) -> String {
    let mut output = String::new();
    let mut last_was_sep = false;
    for ch in input.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            output.push(ch);
            last_was_sep = false;
        } else if !last_was_sep && !output.is_empty() {
            output.push('_');
            last_was_sep = true;
        }
    }
    while output.ends_with('_') {
        output.pop();
    }
    if output.is_empty() {
        "unnamed".to_owned()
    } else {
        output
    }
}

pub fn exposed_tool_name(server: &str, tool: &str) -> String {
    format!(
        "mcp__{}__{}",
        sanitize_component(server),
        sanitize_component(tool)
    )
}

fn unique_tool_name(used: &mut HashSet<String>, server: &str, tool: &str) -> String {
    let base = exposed_tool_name(server, tool);
    if used.insert(base.clone()) {
        return base;
    }
    let mut index = 2;
    loop {
        let candidate = format!("{base}_{index}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        index += 1;
    }
}

fn leak(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

#[cfg(test)]
mod tests {
    use rmcp::model::{CallToolResult, Content};
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_mcp_servers_config() {
        let config: McpConfig = serde_json::from_str(
            r#"{
                "mcpServers": {
                    "filesystem": {
                        "command": "npx",
                        "args": ["-y", "pkg"],
                        "env": {"A": "B"}
                    }
                }
            }"#,
        )
        .unwrap();
        let server = config.mcp_servers.get("filesystem").unwrap();
        assert_eq!(server.command, "npx");
        assert_eq!(server.args, ["-y", "pkg"]);
        assert_eq!(server.env.get("A").unwrap(), "B");
    }

    #[test]
    fn sanitizes_names() {
        assert_eq!(
            exposed_tool_name("File System", "Read.Path"),
            "mcp__file_system__read_path"
        );
        assert_eq!(exposed_tool_name("한글", "!!!"), "mcp__unnamed__unnamed");
    }

    #[test]
    fn unique_names_are_deterministic() {
        let mut used = HashSet::new();
        assert_eq!(unique_tool_name(&mut used, "a-b", "c"), "mcp__a_b__c");
        assert_eq!(unique_tool_name(&mut used, "a_b", "c"), "mcp__a_b__c_2");
    }

    #[test]
    fn converts_text_and_structured_result() {
        let output =
            convert_result("tool", CallToolResult::structured(json!({"ok": true}))).unwrap();
        assert_eq!(
            output.as_text().unwrap(),
            "{\"ok\":true}\nstructuredContent: {\"ok\":true}"
        );
    }

    #[test]
    fn converts_error_result_to_error() {
        let result = convert_result("tool", CallToolResult::error(vec![Content::text("bad")]));
        assert!(matches!(result, Err(err) if err.to_string().contains("bad")));
    }
}
