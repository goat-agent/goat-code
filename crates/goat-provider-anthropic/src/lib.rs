use std::collections::HashMap;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use goat_auth::{
    Credential, CredentialKey, CredentialStore, Pkce, TokenSet, ensure_valid, random_state,
};
use goat_provider::{
    AuthMethod, Capabilities, ContentBlock, Effort, Message, MessageRole, Model, Provider,
    ProviderId, RateLimitSnapshot, RateWindow, Request, StreamEvent, Usage,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{sync::mpsc, task::JoinHandle};

pub const PROVIDER_ID: &str = "anthropic";
const BASE_URL: &str = "https://api.anthropic.com/v1";
const ENV_VAR: &str = "ANTHROPIC_API_KEY";
const VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 16384;

const OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const OAUTH_AUTHORIZE: &str = "https://claude.ai/oauth/authorize";
const OAUTH_TOKEN: &str = "https://platform.claude.com/v1/oauth/token";
const OAUTH_SCOPE: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
const OAUTH_BETA: &str = "oauth-2025-04-20,claude-code-20250219";
const OAUTH_USER_AGENT: &str = "claude-cli/2.1.119 (external, cli)";
const OAUTH_TOKEN_UA: &str = "axios/1.13.6";
const CLAUDE_CODE_SYSTEM: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

#[derive(Debug, thiserror::Error)]
pub enum AnthropicAuthError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("url error: {0}")]
    Url(String),
    #[error("token error: {0}")]
    Token(String),
    #[error("auth error: {0}")]
    Auth(#[from] goat_auth::AuthError),
}

const ANTHROPIC_CONTEXT_WINDOW: u32 = 200_000;

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
    store: CredentialStore,
    key: CredentialKey,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(store: CredentialStore, key: CredentialKey) -> Self {
        Self {
            base_url: BASE_URL.to_owned(),
            store,
            key,
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
    AnthropicProvider::new(store.clone(), key)
}

enum Auth {
    ApiKey(String),
    OAuth(String),
}

fn authorize_url(
    challenge: &str,
    state: &str,
    redirect_uri: &str,
) -> Result<String, AnthropicAuthError> {
    reqwest::Url::parse_with_params(
        OAUTH_AUTHORIZE,
        &[
            ("code", "true"),
            ("client_id", OAUTH_CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", redirect_uri),
            ("scope", OAUTH_SCOPE),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("state", state),
        ],
    )
    .map(|url| url.to_string())
    .map_err(|err| AnthropicAuthError::Url(err.to_string()))
}

async fn do_login(status: &mpsc::Sender<String>) -> Result<TokenSet, AnthropicAuthError> {
    let pkce = Pkce::generate();
    let state = random_state();
    let (listener, port) = goat_auth::bind_loopback().await?;
    let redirect = format!("http://localhost:{port}/callback");
    let url = authorize_url(&pkce.challenge, &state, &redirect)?;
    let _ = status
        .send(format!(
            "opening browser to sign in\u{2026} if it does not open, visit:\n{url}"
        ))
        .await;
    let _ = open::that(&url);
    let code = goat_auth::capture_on(listener, &state).await?;
    exchange_code(&code, &pkce.verifier, &state, &redirect).await
}

fn auth_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("reqwest client")
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

async fn exchange_code(
    code: &str,
    verifier: &str,
    state: &str,
    redirect_uri: &str,
) -> Result<TokenSet, AnthropicAuthError> {
    let response = auth_client()
        .post(OAUTH_TOKEN)
        .header("Accept", "application/json, text/plain, */*")
        .header("User-Agent", OAUTH_TOKEN_UA)
        .json(&json!({
            "grant_type": "authorization_code",
            "code": code,
            "state": state,
            "client_id": OAUTH_CLIENT_ID,
            "redirect_uri": redirect_uri,
            "code_verifier": verifier,
        }))
        .send()
        .await?;
    parse_token_response(response)
        .await
        .map(|t| TokenSet::from_parts(t.access_token, t.refresh_token, t.expires_in, None))
}

async fn do_refresh(refresh_token: String) -> Result<TokenSet, String> {
    let response = auth_client()
        .post(OAUTH_TOKEN)
        .header("Accept", "application/json, text/plain, */*")
        .header("User-Agent", OAUTH_TOKEN_UA)
        .json(&json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": OAUTH_CLIENT_ID,
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    parse_token_response(response)
        .await
        .map(|t| {
            TokenSet::from_parts(
                t.access_token,
                t.refresh_token,
                t.expires_in,
                Some(&refresh_token),
            )
        })
        .map_err(|e| e.to_string())
}

async fn parse_token_response(
    response: reqwest::Response,
) -> Result<TokenResponse, AnthropicAuthError> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(AnthropicAuthError::Token(format!("{status}: {body}")));
    }
    response.json().await.map_err(AnthropicAuthError::Http)
}

async fn current_auth(store: &CredentialStore, key: &CredentialKey) -> Option<Auth> {
    match store.resolve(key, Some(ENV_VAR))? {
        Credential::ApiKey(secret) => Some(Auth::ApiKey(secret.expose().to_owned())),
        Credential::OAuth(tokens) => {
            let tokens = ensure_valid(tokens, store, key, do_refresh).await?;
            Some(Auth::OAuth(tokens.access_token.expose().to_owned()))
        }
    }
}

fn build_system(system: &str, oauth: bool) -> Option<serde_json::Value> {
    if oauth {
        let mut blocks = vec![json!({ "type": "text", "text": CLAUDE_CODE_SYSTEM })];
        if !system.is_empty() {
            blocks.push(json!({ "type": "text", "text": system }));
        }
        Some(serde_json::Value::Array(blocks))
    } else if system.is_empty() {
        None
    } else {
        Some(serde_json::Value::String(system.to_owned()))
    }
}

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<serde_json::Value>,
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

fn parse_anthropic_unified_ratelimits(
    headers: &reqwest::header::HeaderMap,
) -> Option<RateLimitSnapshot> {
    let mut windows = Vec::new();

    if let Some(window) = parse_unified_window(headers, "5h", "5h") {
        windows.push(window);
    }
    if let Some(window) = parse_unified_window(headers, "7d", "weekly") {
        windows.push(window);
    }

    if windows.is_empty() {
        None
    } else {
        Some(RateLimitSnapshot { windows })
    }
}

fn parse_unified_window(
    headers: &reqwest::header::HeaderMap,
    period: &str,
    label: &str,
) -> Option<RateWindow> {
    let util_key = format!("anthropic-ratelimit-unified-{period}-utilization");
    let reset_key = format!("anthropic-ratelimit-unified-{period}-reset");

    let raw = headers.get(&util_key).and_then(|v| v.to_str().ok())?;
    let fraction: f32 = raw.trim_end_matches('%').parse().ok()?;
    #[allow(clippy::cast_possible_truncation)]
    let used_percent = if raw.ends_with('%') {
        fraction
    } else {
        fraction * 100.0
    };

    let resets_at = headers
        .get(&reset_key)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok().or_else(|| parse_rfc3339_unix(s)));

    Some(RateWindow {
        label: label.to_owned(),
        used_percent,
        resets_at,
    })
}

fn parse_rfc3339_unix(s: &str) -> Option<i64> {
    let s = if let Some(pos) = s.find('+') {
        &s[..pos]
    } else {
        s.trim_end_matches('Z')
    };
    let (date, time) = s.split_once('T')?;
    let mut dp = date.splitn(3, '-');
    let year: i64 = dp.next()?.parse().ok()?;
    let month: i64 = dp.next()?.parse().ok()?;
    let day: i64 = dp.next()?.parse().ok()?;
    let mut tp = time.splitn(3, ':');
    let hour: i64 = tp.next()?.parse().ok()?;
    let min: i64 = tp.next()?.parse().ok()?;
    #[allow(clippy::cast_possible_truncation)]
    let sec: i64 = tp.next()?.trim_end_matches('Z').parse::<f64>().ok()? as i64;
    Some(gregorian_to_unix(year, month, day, hour, min, sec))
}

fn gregorian_to_unix(year: i64, month: i64, day: i64, h: i64, m: i64, s: i64) -> i64 {
    let (y, mo) = if month <= 2 {
        (year - 1, month + 9)
    } else {
        (year, month - 3)
    };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * mo + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + h * 3_600 + m * 60 + s
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

fn message_json(role: &str, message: &Message) -> serde_json::Value {
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

#[allow(clippy::struct_field_names)]
#[derive(Default, Deserialize)]
struct MessageStartUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: u32,
}

#[derive(Deserialize)]
struct MessageStart {
    message: MessageStartMessage,
}

#[derive(Deserialize)]
struct MessageStartMessage {
    #[serde(default)]
    usage: MessageStartUsage,
}

#[derive(Default, Deserialize)]
struct MessageDeltaUsage {
    #[serde(default)]
    output_tokens: u32,
}

#[derive(Deserialize)]
struct MessageDelta {
    usage: Option<MessageDeltaUsage>,
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

fn split_request(req: &Request) -> (String, Vec<serde_json::Value>, Vec<serde_json::Value>) {
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

#[allow(clippy::too_many_lines)]
async fn stream_messages(response: reqwest::Response, events: &mpsc::Sender<StreamEvent>) {
    let mut stream = response.bytes_stream().eventsource();
    let mut tool_calls: HashMap<u32, (String, String, String)> = HashMap::new();
    let mut usage = Usage::default();
    while let Some(event) = stream.next().await {
        match event {
            Ok(event) => match event.event.as_str() {
                "message_start" => {
                    if let Ok(start) = serde_json::from_str::<MessageStart>(&event.data) {
                        let u = &start.message.usage;
                        usage.input_tokens = u.input_tokens
                            + u.cache_creation_input_tokens
                            + u.cache_read_input_tokens;
                        usage.cache_read_tokens = u.cache_read_input_tokens;
                        usage.cache_write_tokens = u.cache_creation_input_tokens;
                    }
                }
                "message_delta" => {
                    if let Ok(delta) = serde_json::from_str::<MessageDelta>(&event.data)
                        && let Some(u) = delta.usage
                    {
                        usage.output_tokens = u.output_tokens;
                    }
                }
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
                                        .send(StreamEvent::RedactedThinking { data })
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
                        if events.send(StreamEvent::TextDelta { text }).await.is_err() {
                            return;
                        }
                    } else if let Some(text) = parse_thinking_delta(&event.data) {
                        if events
                            .send(StreamEvent::ThinkingDelta { text })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    } else if let Some(signature) = parse_signature_delta(&event.data) {
                        if events
                            .send(StreamEvent::ThinkingSignature { signature })
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
                            .send(StreamEvent::ToolCall { id, name, input })
                            .await
                            .is_err()
                    {
                        return;
                    }
                }
                "message_stop" => {
                    let _ = events.send(StreamEvent::Usage { usage }).await;
                    break;
                }
                "error" => {
                    let _ = events
                        .send(StreamEvent::Failed {
                            message: event.data,
                        })
                        .await;
                    return;
                }
                _ => {}
            },
            Err(err) => {
                let _ = events
                    .send(StreamEvent::Failed {
                        message: err.to_string(),
                    })
                    .await;
                return;
            }
        }
    }
    let _ = events.send(StreamEvent::Completed).await;
}

impl Provider for AnthropicProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from(PROVIDER_ID)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tools: true,
            auth: AuthMethod::ApiKeyOrOAuth,
        }
    }

    fn stream(&self, req: Request, tx: mpsc::Sender<StreamEvent>) -> JoinHandle<()> {
        let client = self.client.clone();
        let url = format!("{}/messages", self.base_url);
        let store = self.store.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let Some(auth) = current_auth(&store, &key).await else {
                let _ = tx
                    .send(StreamEvent::Failed {
                        message: "not logged in to anthropic".to_owned(),
                    })
                    .await;
                return;
            };
            let (system, messages, tools) = split_request(&req);
            let cfg = thinking_config(&req.model, req.effort);
            let oauth = matches!(auth, Auth::OAuth(_));
            let body = MessagesRequest {
                model: &req.model,
                max_tokens: cfg.max_tokens,
                stream: true,
                system: build_system(&system, oauth),
                messages,
                tools,
                thinking: cfg.thinking,
                output_config: cfg.output_config,
            };
            let builder = client
                .post(&url)
                .header("anthropic-version", VERSION)
                .json(&body);
            let builder = match &auth {
                Auth::ApiKey(api_key) => builder.header("x-api-key", api_key),
                Auth::OAuth(access) => builder
                    .bearer_auth(access)
                    .header("anthropic-beta", OAUTH_BETA)
                    .header("user-agent", OAUTH_USER_AGENT)
                    .header("x-app", "cli"),
            };
            let resp = match builder.send().await {
                Ok(resp) => resp,
                Err(err) => {
                    let _ = tx
                        .send(StreamEvent::Failed {
                            message: err.to_string(),
                        })
                        .await;
                    return;
                }
            };
            if !resp.status().is_success() {
                let status = resp.status();
                let detail = resp.text().await.unwrap_or_default();
                let _ = tx
                    .send(StreamEvent::Failed {
                        message: format!("{status}: {detail}"),
                    })
                    .await;
                return;
            }
            if matches!(auth, Auth::OAuth(_))
                && let Some(snapshot) = parse_anthropic_unified_ratelimits(resp.headers())
            {
                let _ = tx.send(StreamEvent::RateLimits { snapshot }).await;
            }
            stream_messages(resp, &tx).await;
        })
    }

    fn authenticated(&self) -> bool {
        self.store.resolve(&self.key, Some(ENV_VAR)).is_some()
    }

    fn validate(&self) -> JoinHandle<Result<(), String>> {
        let client = self.client.clone();
        let url = format!("{}/models", self.base_url);
        let store = self.store.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let auth = current_auth(&store, &key)
                .await
                .ok_or_else(|| "no credentials".to_owned())?;
            let api_key = match auth {
                Auth::OAuth(_) => return Ok(()),
                Auth::ApiKey(api_key) => api_key,
            };
            let resp = client
                .get(&url)
                .header("anthropic-version", VERSION)
                .header("x-api-key", api_key)
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

    fn context_window(&self, _model: &str) -> Option<u32> {
        Some(ANTHROPIC_CONTEXT_WINDOW)
    }

    fn catalog(&self) -> &'static [&'static str] {
        CATALOG
    }

    fn efforts(&self, model: &str) -> Vec<Effort> {
        anthropic_efforts(model)
    }

    fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
        let client = self.client.clone();
        let url = format!("{}/models", self.base_url);
        let store = self.store.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let Some(auth) = current_auth(&store, &key).await else {
                return;
            };
            let api_key = match auth {
                Auth::OAuth(_) => {
                    for &id in CATALOG {
                        if out.send(Model { id: id.to_owned() }).await.is_err() {
                            return;
                        }
                    }
                    return;
                }
                Auth::ApiKey(api_key) => api_key,
            };
            let Ok(resp) = client
                .get(&url)
                .header("anthropic-version", VERSION)
                .header("x-api-key", api_key)
                .send()
                .await
            else {
                return;
            };
            let Ok(models) = resp.json::<ModelsResponse>().await else {
                return;
            };
            for model in models.data {
                if out.send(Model { id: model.id }).await.is_err() {
                    return;
                }
            }
        })
    }

    fn login(&self, status: mpsc::Sender<String>) -> JoinHandle<Result<TokenSet, String>> {
        tokio::spawn(async move { do_login(&status).await.map_err(|e| e.to_string()) })
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
    fn authorize_url_carries_pkce_and_scope() {
        let url = super::authorize_url("CHAL", "STATE", "http://localhost:1234/callback").unwrap();
        assert!(url.contains("client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e"));
        assert!(url.contains("code_challenge=CHAL"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=STATE"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("user%3Ainference"));
        assert!(url.contains("org%3Acreate_api_key"));
        assert!(url.contains("user%3Afile_upload"));
        assert!(url.contains("localhost%3A1234%2Fcallback"));
    }

    #[test]
    fn oauth_system_prepends_claude_code_identity() {
        let blocks = super::build_system("be helpful", true).unwrap();
        assert_eq!(blocks[0]["text"], super::CLAUDE_CODE_SYSTEM);
        assert_eq!(blocks[1]["text"], "be helpful");
        let plain = super::build_system("be helpful", false).unwrap();
        assert_eq!(plain, serde_json::Value::String("be helpful".to_owned()));
        assert!(super::build_system("", false).is_none());
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

    #[test]
    fn no_effort_disables_thinking() {
        let none = super::thinking_config("claude-opus-4-8", None);
        assert!(none.thinking.is_none());
        assert!(none.output_config.is_none());
    }
}
