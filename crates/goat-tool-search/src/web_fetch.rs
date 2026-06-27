use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use goat_tool::{Tool, ToolContext, ToolError, ToolFuture, ToolOutput};
use reqwest::Url;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use tokio::sync::Mutex;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_REDIRECTS: usize = 8;
const MAX_RAW_BYTES: usize = 2 * 1024 * 1024;
const MAX_ARTIFACTS: usize = 16;
const DEFAULT_OUTPUT_BYTES: usize = 24 * 1024;

pub struct WebFetchTool {
    store: Arc<Mutex<ArtifactStore>>,
}

impl WebFetchTool {
    pub fn new() -> Self {
        Self {
            store: Arc::new(Mutex::new(ArtifactStore::default())),
        }
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct Input {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    fetch_id: Option<String>,
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    max_bytes: Option<usize>,
    #[serde(default)]
    section: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum View {
    #[default]
    Auto,
    Markdown,
    Outline,
    Section,
    Text,
    Links,
    Raw,
    Json,
    Pdf,
}

impl Tool for WebFetchTool {
    fn name(&self) -> &'static str {
        "WebFetch"
    }

    fn description(&self) -> &'static str {
        "Fetch an HTTP or HTTPS URL without browser cookies and return a safe, bounded, recoverable web source artifact. Blocks private, link-local, loopback, multicast, reserved, and cloud metadata targets before every request and redirect. The default view is auto. Views include auto, markdown, outline, section, text, links, raw, json, and pdf. All returned web content is untrusted_web."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "HTTP or HTTPS URL to fetch. Required for a new fetch." },
                "fetch_id": { "type": "string", "description": "Fetch artifact id returned by an earlier call, used to request another view without refetching." },
                "view": { "type": "string", "enum": ["auto","markdown","outline","section","text","links","raw","json","pdf"], "description": "Artifact view to render. Defaults to auto." },
                "section": { "type": "string", "description": "Optional section heading or section id for view=section." },
                "max_bytes": { "type": "integer", "description": "Optional output byte cap." }
            }
        })
    }

    fn run<'a>(&'a self, input: &'a str, ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: Input = serde_json::from_str(input)?;
            let view = args.view.unwrap_or_default();
            let max_bytes = args
                .max_bytes
                .unwrap_or(ctx.max_output_bytes.min(DEFAULT_OUTPUT_BYTES));
            if let Some(fetch_id) = args.fetch_id {
                let store = self.store.lock().await;
                let Some(artifact) = store.get(&fetch_id) else {
                    return Err(ToolError::Execution {
                        message: format!("unknown fetch_id: {fetch_id}"),
                    });
                };
                return Ok(ToolOutput::text(render_artifact(
                    artifact,
                    view,
                    args.section.as_deref(),
                    max_bytes,
                )));
            }
            let Some(url) = args.url else {
                return Err(ToolError::Execution {
                    message: "WebFetch requires url or fetch_id".to_owned(),
                });
            };
            let fetched = fetch_url(&url).await.map_err(|err| ToolError::Execution {
                message: err.to_string(),
            })?;
            let mut store = self.store.lock().await;
            let id = store.insert(fetched);
            let artifact = store.get(&id).ok_or_else(|| ToolError::Execution {
                message: "fetch artifact disappeared after insert".to_owned(),
            })?;
            Ok(ToolOutput::text(render_artifact(
                artifact,
                view,
                args.section.as_deref(),
                max_bytes,
            )))
        })
    }
}

#[derive(Default)]
struct ArtifactStore {
    next_id: u64,
    order: VecDeque<String>,
    artifacts: HashMap<String, FetchArtifact>,
}

impl ArtifactStore {
    fn insert(&mut self, mut artifact: FetchArtifact) -> String {
        self.next_id = self.next_id.saturating_add(1);
        let id = format!("f{}", self.next_id);
        artifact.fetch_id.clone_from(&id);
        self.order.push_back(id.clone());
        self.artifacts.insert(id.clone(), artifact);
        while self.order.len() > MAX_ARTIFACTS {
            if let Some(old) = self.order.pop_front() {
                self.artifacts.remove(&old);
            }
        }
        id
    }

    fn get(&self, id: &str) -> Option<&FetchArtifact> {
        self.artifacts.get(id)
    }
}

#[derive(Debug)]
struct FetchArtifact {
    fetch_id: String,
    requested_url: String,
    final_url: String,
    redirects: Vec<String>,
    status: u16,
    content_type: String,
    retrieved_at: String,
    content_hash: String,
    raw_bytes: Vec<u8>,
    raw_byte_size: usize,
    truncated: bool,
    page_type: PageType,
    extract_method: &'static str,
    warnings: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageType {
    Html,
    Json,
    Text,
    Pdf,
    Binary,
}

#[derive(Debug, thiserror::Error)]
enum FetchError {
    #[error("invalid url: {0}")]
    Url(String),
    #[error("unsupported url scheme: {0}")]
    Scheme(String),
    #[error("url host is missing")]
    MissingHost,
    #[error("blocked unsafe network target: {0}")]
    UnsafeTarget(String),
    #[error("dns lookup failed for {0}: {1}")]
    Dns(String, std::io::Error),
    #[error("request failed: {0}")]
    Request(reqwest::Error),
    #[error("redirect limit exceeded")]
    RedirectLimit,
    #[error("redirect target missing Location header")]
    RedirectMissingLocation,
    #[error("invalid redirect target: {0}")]
    RedirectUrl(String),
}

async fn fetch_url(raw_url: &str) -> Result<FetchArtifact, FetchError> {
    let requested = parse_safe_url(raw_url)?;
    let mut current = requested.clone();
    let mut redirects = Vec::new();
    for _ in 0..=MAX_REDIRECTS {
        let response = request_once(&current).await?;
        let status = response.status();
        if status.is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .ok_or(FetchError::RedirectMissingLocation)?
                .to_str()
                .map_err(|err| FetchError::RedirectUrl(err.to_string()))?;
            let next = current
                .join(location)
                .map_err(|err| FetchError::RedirectUrl(err.to_string()))?;
            let next = parse_safe_url(next.as_str())?;
            redirects.push(next.to_string());
            current = next;
            continue;
        }
        let status_code = status.as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_owned();
        let (raw_bytes, truncated) = read_bounded(response).await?;
        let raw_byte_size = raw_bytes.len();
        let page_type = classify(&content_type, &current);
        let mut warnings = Vec::new();
        if truncated {
            warnings.push("content_truncated");
        }
        if page_type == PageType::Html {
            warnings.push("scripts_not_executed");
            if looks_login_required(&raw_bytes) {
                warnings.push("login_required");
            }
            if looks_javascript_required(&raw_bytes) {
                warnings.push("javascript_required");
            }
        }
        if page_type == PageType::Pdf {
            warnings.push("pdf_text_not_extracted");
        }
        if !status.is_success() {
            warnings.push("http_status_not_success");
        }
        let content_hash = sha256_hex(&raw_bytes);
        return Ok(FetchArtifact {
            fetch_id: String::new(),
            requested_url: requested.to_string(),
            final_url: current.to_string(),
            redirects,
            status: status_code,
            content_type,
            retrieved_at: now_string(),
            content_hash,
            raw_bytes,
            raw_byte_size,
            truncated,
            page_type,
            extract_method: extract_method(page_type),
            warnings,
        });
    }
    Err(FetchError::RedirectLimit)
}

async fn request_once(url: &Url) -> Result<reqwest::Response, FetchError> {
    let host = url.host_str().ok_or(FetchError::MissingHost)?.to_owned();
    let addrs = resolve_safe_addrs(url).await?;
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(&host, &addrs)
        .user_agent("goat-code WebFetch")
        .build()
        .map_err(FetchError::Request)?
        .get(url.clone())
        .send()
        .await
        .map_err(FetchError::Request)
}

async fn read_bounded(mut response: reqwest::Response) -> Result<(Vec<u8>, bool), FetchError> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = response.chunk().await.map_err(FetchError::Request)? {
        let remaining = MAX_RAW_BYTES.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
        if bytes.len() == MAX_RAW_BYTES {
            truncated = true;
            break;
        }
    }
    Ok((bytes, truncated))
}

fn parse_safe_url(raw: &str) -> Result<Url, FetchError> {
    let url = Url::parse(raw).map_err(|err| FetchError::Url(err.to_string()))?;
    match url.scheme() {
        "http" | "https" => {}
        other => return Err(FetchError::Scheme(other.to_owned())),
    }
    let host = url.host_str().ok_or(FetchError::MissingHost)?;
    if let Ok(ip) = host.parse::<IpAddr>() {
        ensure_safe_ip(ip)?;
    }
    Ok(url)
}

async fn resolve_safe_addrs(url: &Url) -> Result<Vec<SocketAddr>, FetchError> {
    let host = url.host_str().ok_or(FetchError::MissingHost)?.to_owned();
    if let Ok(ip) = host.parse::<IpAddr>() {
        ensure_safe_ip(ip)?;
        return Ok(vec![SocketAddr::new(
            ip,
            url.port_or_known_default().unwrap_or(443),
        )]);
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|err| FetchError::Dns(host.clone(), err))?
        .collect();
    if addrs.is_empty() {
        return Err(FetchError::UnsafeTarget(format!(
            "{host} resolved to no addresses"
        )));
    }
    for addr in &addrs {
        ensure_safe_ip(addr.ip())?;
    }
    Ok(addrs)
}

fn ensure_safe_ip(ip: IpAddr) -> Result<(), FetchError> {
    let blocked = match ip {
        IpAddr::V4(ip) => unsafe_v4(ip),
        IpAddr::V6(ip) => unsafe_v6(ip),
    };
    if blocked {
        Err(FetchError::UnsafeTarget(ip.to_string()))
    } else {
        Ok(())
    }
}

fn unsafe_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.is_unspecified()
        || o[0] == 0
        || o[0] >= 224
        || o[0] == 127
        || (o[0] == 100 && (64..=127).contains(&o[1]))
        || (o[0] == 169 && o[1] == 254)
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 192 && o[1] == 0 && o[2] == 2)
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
        || (o[0] == 198 && o[1] == 51 && o[2] == 100)
        || (o[0] == 203 && o[1] == 0 && o[2] == 113)
}

fn unsafe_v6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return unsafe_v4(mapped);
    }
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (ip.segments()[0] & 0xffc0) == 0xfe80
        || (ip.segments()[0] & 0xfe00) == 0xfc00
        || (ip.segments()[0] & 0xff00) == 0xff00
        || (ip.segments()[0] == 0x2001 && ip.segments()[1] == 0x0db8)
}

fn classify(content_type: &str, url: &Url) -> PageType {
    let lower = content_type.to_ascii_lowercase();
    if lower.contains("html") {
        PageType::Html
    } else if lower.contains("json") {
        PageType::Json
    } else if lower.starts_with("text/") || lower.contains("xml") {
        PageType::Text
    } else if lower.contains("pdf") || url.path().to_ascii_lowercase().ends_with(".pdf") {
        PageType::Pdf
    } else {
        PageType::Binary
    }
}

fn extract_method(page_type: PageType) -> &'static str {
    match page_type {
        PageType::Html => "html_structure",
        PageType::Json => "json_pretty",
        PageType::Text => "plain_text",
        PageType::Pdf => "pdf_metadata_only",
        PageType::Binary => "metadata_only",
    }
}

fn render_artifact(
    artifact: &FetchArtifact,
    view: View,
    section: Option<&str>,
    max_bytes: usize,
) -> String {
    let chosen = if view == View::Auto {
        match artifact.page_type {
            PageType::Html => View::Markdown,
            PageType::Json => View::Json,
            PageType::Text => View::Text,
            PageType::Pdf | PageType::Binary => View::Outline,
        }
    } else {
        view
    };
    let mut out = envelope(artifact, chosen);
    match chosen {
        View::Auto => {}
        View::Markdown => render_markdown(artifact, &mut out),
        View::Outline => render_outline(artifact, &mut out),
        View::Section => render_section(artifact, section, &mut out),
        View::Text => render_text(artifact, &mut out),
        View::Links => render_links(artifact, &mut out),
        View::Raw => render_raw(artifact, &mut out),
        View::Json => render_json(artifact, &mut out),
        View::Pdf => render_pdf(artifact, &mut out),
    }
    cap(out, max_bytes)
}

fn envelope(artifact: &FetchArtifact, view: View) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "fetch_id: {}", artifact.fetch_id);
    let _ = writeln!(out, "requested_url: {}", artifact.requested_url);
    let _ = writeln!(out, "final_url: {}", artifact.final_url);
    let _ = writeln!(out, "status: {}", artifact.status);
    let _ = writeln!(out, "content_type: {}", artifact.content_type);
    let _ = writeln!(out, "retrieved_at: {}", artifact.retrieved_at);
    out.push_str("source_trust: untrusted_web\n");
    let _ = writeln!(out, "page_type: {}", page_type_name(artifact.page_type));
    let _ = writeln!(out, "extract_method: {}", artifact.extract_method);
    let _ = writeln!(out, "view: {}", view_name(view));
    let _ = writeln!(out, "content_hash: sha256:{}", artifact.content_hash);
    let _ = writeln!(out, "raw_byte_size: {}", artifact.raw_byte_size);
    let _ = writeln!(out, "truncated: {}", artifact.truncated);
    if !artifact.redirects.is_empty() {
        out.push_str("redirects:\n");
        for redirect in &artifact.redirects {
            let _ = writeln!(out, "- {redirect}");
        }
    }
    out.push_str("warnings:\n");
    out.push_str("- page_content_untrusted\n");
    if artifact.warnings.is_empty() {
        out.push_str("- none\n");
    } else {
        for warning in &artifact.warnings {
            let _ = writeln!(out, "- {warning}");
        }
    }
    out.push_str(
        "available_views:\n- auto\n- markdown\n- outline\n- section\n- text\n- links\n- raw\n- json\n- pdf\n",
    );
    out.push('\n');
    out
}

fn render_markdown(artifact: &FetchArtifact, out: &mut String) {
    out.push_str("untrusted_content_markdown:\n");
    match artifact.page_type {
        PageType::Html => {
            let html = decode_lossy(&artifact.raw_bytes);
            let title = extract_title(&html);
            if let Some(title) = title {
                let _ = writeln!(out, "# {}\n", clean_text(&title));
            }
            for item in extract_headings_and_paragraphs(&html).into_iter().take(80) {
                out.push_str(&item);
                out.push('\n');
            }
        }
        PageType::Text | PageType::Json => render_text(artifact, out),
        PageType::Pdf => {
            out.push_str("[pdf content not extracted; use a browser or dedicated PDF reader]\n");
        }
        PageType::Binary => out.push_str("[binary content not rendered]\n"),
    }
}

fn render_outline(artifact: &FetchArtifact, out: &mut String) {
    out.push_str("outline:\n");
    match artifact.page_type {
        PageType::Html => {
            let html = decode_lossy(&artifact.raw_bytes);
            let mut count = 0usize;
            for heading in extract_headings(&html).into_iter().take(80) {
                let _ = writeln!(out, "- {heading}");
                count += 1;
            }
            if count == 0 {
                out.push_str("- no headings detected\n");
            }
        }
        PageType::Json => out.push_str("- json document\n"),
        PageType::Text => out.push_str("- text document\n"),
        PageType::Pdf => out.push_str("- pdf document metadata only\n"),
        PageType::Binary => out.push_str("- binary document metadata only\n"),
    }
}

fn render_text(artifact: &FetchArtifact, out: &mut String) {
    out.push_str("untrusted_content_text:\n");
    let text = decode_lossy(&artifact.raw_bytes);
    match artifact.page_type {
        PageType::Html => out.push_str(&html_to_text(&text)),
        _ => out.push_str(&text),
    }
    out.push('\n');
}

fn render_section(artifact: &FetchArtifact, section: Option<&str>, out: &mut String) {
    out.push_str("section:\n");
    let Some(section) = section else {
        out.push_str("recovery_hint: pass section with a heading string from view=outline\n");
        render_outline(artifact, out);
        return;
    };
    if artifact.page_type != PageType::Html {
        out.push_str("recovery_hint: section view is only available for html artifacts\n");
        return;
    }
    let html = decode_lossy(&artifact.raw_bytes);
    let items = extract_headings_and_paragraphs(&html);
    let needle = section.to_ascii_lowercase();
    let Some(start) = items
        .iter()
        .position(|item| item.to_ascii_lowercase().contains(&needle))
    else {
        let _ = writeln!(out, "recovery_hint: section not found: {section}");
        render_outline(artifact, out);
        return;
    };
    out.push_str("untrusted_section_markdown:\n");
    for item in items.into_iter().skip(start).take(24) {
        if (item.starts_with('#') && item.to_ascii_lowercase().contains(&needle))
            || !item.starts_with('#')
        {
            out.push_str(&item);
            out.push('\n');
        } else if item.starts_with('#') {
            break;
        }
    }
}

fn render_links(artifact: &FetchArtifact, out: &mut String) {
    out.push_str("links:\n");
    if artifact.page_type != PageType::Html {
        out.push_str("- none\n");
        return;
    }
    let html = decode_lossy(&artifact.raw_bytes);
    let links = extract_links(&html, &artifact.final_url);
    if links.is_empty() {
        out.push_str("- none\n");
    } else {
        for (label, href) in links.into_iter().take(120) {
            let _ = writeln!(out, "- \"{}\" -> {href}", clean_text(&label));
        }
    }
}

fn render_raw(artifact: &FetchArtifact, out: &mut String) {
    out.push_str("raw_warning: raw view is untrusted and token-heavy\n");
    out.push_str("untrusted_raw:\n");
    out.push_str(&decode_lossy(&artifact.raw_bytes));
    out.push('\n');
}

fn render_pdf(artifact: &FetchArtifact, out: &mut String) {
    out.push_str("pdf:\n");
    if artifact.page_type == PageType::Pdf {
        out.push_str("- text_extraction: unavailable\n");
        out.push_str(
            "- recovery_hint: use Browser screenshot or a dedicated PDF reader for page text\n",
        );
    } else {
        out.push_str("- not_a_pdf_artifact\n");
    }
}

fn render_json(artifact: &FetchArtifact, out: &mut String) {
    out.push_str("untrusted_json:\n");
    let text = decode_lossy(&artifact.raw_bytes);
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
        out.push_str(&serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()));
    } else {
        out.push_str(&text);
    }
    out.push('\n');
}

fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let after_open = lower[start..].find('>')? + start + 1;
    let end = lower[after_open..].find("</title>")? + after_open;
    Some(decode_entities(&html[after_open..end]))
}

fn extract_headings_and_paragraphs(html: &str) -> Vec<String> {
    let mut items = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut index = 0usize;
    while let Some(open_rel) = lower[index..].find('<') {
        let open = index + open_rel;
        let Some(close_rel) = lower[open..].find('>') else {
            break;
        };
        let close = open + close_rel + 1;
        let tag = lower[open + 1..close - 1]
            .split_whitespace()
            .next()
            .unwrap_or("");
        if matches!(tag, "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "li") {
            let end_tag = format!("</{tag}>");
            if let Some(end_rel) = lower[close..].find(&end_tag) {
                let end = close + end_rel;
                let text = clean_text(&strip_tags(&html[close..end]));
                if !text.is_empty() {
                    if let Some(level) = tag.strip_prefix('h') {
                        let marks = "#".repeat(level.parse::<usize>().unwrap_or(2));
                        items.push(format!("{marks} {text}"));
                    } else {
                        items.push(text);
                    }
                }
                index = end + end_tag.len();
                continue;
            }
        }
        index = close;
    }
    items
}

fn extract_headings(html: &str) -> Vec<String> {
    extract_headings_and_paragraphs(html)
        .into_iter()
        .filter(|item| item.starts_with('#'))
        .collect()
}

fn extract_links(html: &str, base: &str) -> Vec<(String, String)> {
    let Ok(base) = Url::parse(base) else {
        return Vec::new();
    };
    let lower = html.to_ascii_lowercase();
    let mut index = 0usize;
    let mut links = Vec::new();
    while let Some(open_rel) = lower[index..].find("<a") {
        let open = index + open_rel;
        let Some(close_rel) = lower[open..].find('>') else {
            break;
        };
        let close = open + close_rel + 1;
        let attrs = &html[open..close];
        if let Some(href) = attr_value(attrs, "href")
            && let Ok(url) = base.join(&href)
        {
            let end = lower[close..]
                .find("</a>")
                .map_or(close, |end_rel| close + end_rel);
            let label = clean_text(&strip_tags(&html[close..end]));
            links.push((label, url.to_string()));
        }
        index = close;
    }
    links
}

fn attr_value(attrs: &str, name: &str) -> Option<String> {
    let lower = attrs.to_ascii_lowercase();
    let needle = format!("{name}=");
    let start = lower.find(&needle)? + needle.len();
    let quote = attrs[start..].chars().next()?;
    if quote == '"' || quote == '\'' {
        let rest = &attrs[start + quote.len_utf8()..];
        let end = rest.find(quote)?;
        Some(rest[..end].to_owned())
    } else {
        let rest = &attrs[start..];
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        Some(rest[..end].trim_end_matches('>').to_owned())
    }
}

fn html_to_text(html: &str) -> String {
    clean_text(&strip_tags(html))
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    decode_entities(&out)
}

fn decode_entities(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn clean_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn page_type_name(page_type: PageType) -> &'static str {
    match page_type {
        PageType::Html => "html",
        PageType::Json => "json",
        PageType::Text => "text",
        PageType::Pdf => "pdf",
        PageType::Binary => "binary",
    }
}

fn view_name(view: View) -> &'static str {
    match view {
        View::Auto => "auto",
        View::Markdown => "markdown",
        View::Outline => "outline",
        View::Section => "section",
        View::Text => "text",
        View::Links => "links",
        View::Raw => "raw",
        View::Json => "json",
        View::Pdf => "pdf",
    }
}

fn looks_login_required(bytes: &[u8]) -> bool {
    let text = decode_lossy(bytes).to_ascii_lowercase();
    text.contains("login") || text.contains("sign in") || text.contains("signin")
}

fn looks_javascript_required(bytes: &[u8]) -> bool {
    let text = decode_lossy(bytes).to_ascii_lowercase();
    text.contains("enable javascript") || text.contains("javascript required")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn now_string() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    format!("unix:{seconds}")
}

fn cap(mut text: String, max_bytes: usize) -> String {
    if text.len() > max_bytes {
        let boundary = text.floor_char_boundary(max_bytes);
        text.truncate(boundary);
        text.push_str("\n[output truncated]");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{
        PageType, View, classify, ensure_safe_ip, extract_links, parse_safe_url, render_artifact,
        sha256_hex,
    };

    #[test]
    fn rejects_private_ips() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(ensure_safe_ip(ip.parse().unwrap()).is_err(), "{ip}");
        }
    }

    #[test]
    fn accepts_public_ip() {
        assert!(ensure_safe_ip("93.184.216.34".parse().unwrap()).is_ok());
    }

    #[test]
    fn rejects_unsupported_scheme() {
        assert!(parse_safe_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn classifies_content() {
        let url = reqwest::Url::parse("https://example.com/a.pdf").unwrap();
        assert_eq!(classify("text/html", &url), PageType::Html);
        assert_eq!(classify("application/json", &url), PageType::Json);
        assert_eq!(classify("text/plain", &url), PageType::Text);
        assert_eq!(classify("application/octet-stream", &url), PageType::Pdf);
    }

    #[test]
    fn extracts_links_relative_to_final_url() {
        let links = extract_links(
            r#"<a href="/docs"> Docs </a><a href='https://x.test/a'>A</a>"#,
            "https://example.com/base",
        );
        assert_eq!(links[0].1, "https://example.com/docs");
        assert_eq!(links[1].1, "https://x.test/a");
    }

    #[test]
    fn renders_untrusted_envelope() {
        let artifact = super::FetchArtifact {
            fetch_id: "f1".to_owned(),
            requested_url: "https://example.com".to_owned(),
            final_url: "https://example.com".to_owned(),
            redirects: Vec::new(),
            status: 200,
            content_type: "text/html".to_owned(),
            retrieved_at: "unix:1".to_owned(),
            content_hash: sha256_hex(b"<h1>Hello</h1>"),
            raw_bytes: b"<title>T</title><h1>Hello</h1>".to_vec(),
            raw_byte_size: 29,
            truncated: false,
            page_type: PageType::Html,
            extract_method: "html_structure",
            warnings: vec!["scripts_not_executed"],
        };
        let out = render_artifact(&artifact, View::Markdown, None, 4096);
        assert!(out.contains("source_trust: untrusted_web"));
        assert!(out.contains("warnings:"));
        assert!(out.contains("untrusted_content_markdown:"));
        assert!(out.contains("# T"));
    }
}
