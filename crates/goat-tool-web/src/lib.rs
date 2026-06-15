mod ssrf;

use std::{net::IpAddr, sync::Arc, time::Duration};

use futures::StreamExt;
use goat_protocol::ToolDisplay;
use goat_tool::{Tool, ToolContext, ToolError, ToolFuture, ToolOutput, display};
use serde::Deserialize;

const MAX_DOWNLOAD: usize = 5 * 1024 * 1024;
const MAX_OUTPUT: usize = 48 * 1024;
const USER_AGENT: &str = "goat-code/0.1 (+https://github.com/jbj338033/goat-code)";

pub fn all() -> Vec<Box<dyn Tool>> {
    vec![Box::new(WebFetchTool)]
}

pub struct WebFetchTool;

#[derive(Deserialize)]
struct Input {
    url: String,
}

impl Tool for WebFetchTool {
    fn name(&self) -> &'static str {
        "WebFetch"
    }

    fn description(&self) -> &'static str {
        "Fetch a web page over HTTPS and return its content as Markdown. Use for reading documentation, articles, or source files referenced by URL. Private and link-local addresses are refused."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {"type": "string"}
            },
            "required": ["url"]
        })
    }

    fn display_input(&self, input: &str) -> ToolDisplay {
        match serde_json::from_str::<Input>(input) {
            Ok(args) => ToolDisplay::primary(display::flatten(&args.url)),
            Err(_) => display::generic(input),
        }
    }

    fn run<'a>(&'a self, input: &'a str, _ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: Input = serde_json::from_str(input)?;
            let url = normalize_url(&args.url);
            reject_blocked_literal(&url)?;
            fetch(&url).await
        })
    }
}

fn normalize_url(raw: &str) -> String {
    let upgraded = raw
        .strip_prefix("http://")
        .map_or_else(|| raw.to_owned(), |rest| format!("https://{rest}"));
    github_blob_to_raw(&upgraded)
}

fn github_blob_to_raw(url: &str) -> String {
    let Some(rest) = url.strip_prefix("https://github.com/") else {
        return url.to_owned();
    };
    let parts: Vec<&str> = rest.splitn(4, '/').collect();
    if parts.len() == 4 && parts[2] == "blob" {
        format!(
            "https://raw.githubusercontent.com/{}/{}/{}",
            parts[0], parts[1], parts[3]
        )
    } else {
        url.to_owned()
    }
}

fn reject_blocked_literal(url: &str) -> Result<(), ToolError> {
    let authority = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, after)| after);
    let host = if let Some(rest) = host_port.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    if let Ok(ip) = host.parse::<IpAddr>()
        && ssrf::is_blocked(ip)
    {
        return Err(ToolError::Execution {
            message: format!("refusing to fetch a private or local address: {host}"),
        });
    }
    Ok(())
}

fn build_client() -> Result<reqwest::Client, ToolError> {
    let redirect = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error(RedirectBlocked("too many redirects"));
        }
        let blocked = attempt
            .url()
            .host_str()
            .map(|h| h.trim_start_matches('[').trim_end_matches(']').to_owned())
            .and_then(|h| h.parse::<IpAddr>().ok())
            .is_some_and(ssrf::is_blocked);
        if blocked {
            attempt.error(RedirectBlocked("redirect to a private or local address"))
        } else {
            attempt.follow()
        }
    });
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .redirect(redirect)
        .dns_resolver(Arc::new(ssrf::GuardedResolver))
        .build()
        .map_err(|err| ToolError::Execution {
            message: format!("could not build HTTP client: {err}"),
        })
}

#[derive(Debug)]
struct RedirectBlocked(&'static str);

impl std::fmt::Display for RedirectBlocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for RedirectBlocked {}

async fn fetch(url: &str) -> Result<ToolOutput, ToolError> {
    let client = build_client()?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| ToolError::Execution {
            message: format!("request failed: {err}"),
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(ToolError::Execution {
            message: format!("server returned {status}"),
        });
    }
    let is_html = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("html"));

    let mut stream = response.bytes_stream();
    let mut body: Vec<u8> = Vec::new();
    let mut overflowed = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| ToolError::Execution {
            message: format!("download failed: {err}"),
        })?;
        if body.len() + chunk.len() > MAX_DOWNLOAD {
            let room = MAX_DOWNLOAD.saturating_sub(body.len());
            body.extend_from_slice(&chunk[..room]);
            overflowed = true;
            break;
        }
        body.extend_from_slice(&chunk);
    }

    let raw = String::from_utf8_lossy(&body);
    let text = if is_html {
        htmd::convert(&raw).unwrap_or_else(|_| raw.into_owned())
    } else {
        raw.into_owned()
    };

    let mut text = if text.len() > MAX_OUTPUT {
        goat_tool::truncate(text, MAX_OUTPUT)
    } else {
        text
    };
    if overflowed && text.len() <= MAX_OUTPUT {
        text.push_str(goat_tool::TRUNCATION_NOTICE);
    }

    Ok(ToolOutput::text(text))
}

#[cfg(test)]
mod tests {
    use super::{github_blob_to_raw, normalize_url, reject_blocked_literal};

    #[test]
    fn upgrades_http_to_https() {
        assert_eq!(
            normalize_url("http://example.com/x"),
            "https://example.com/x"
        );
    }

    #[test]
    fn rewrites_github_blob() {
        assert_eq!(
            github_blob_to_raw("https://github.com/o/r/blob/main/src/lib.rs"),
            "https://raw.githubusercontent.com/o/r/main/src/lib.rs"
        );
    }

    #[test]
    fn leaves_other_github_urls() {
        assert_eq!(
            github_blob_to_raw("https://github.com/o/r/issues/1"),
            "https://github.com/o/r/issues/1"
        );
    }

    #[test]
    fn rejects_ip_literal_localhost() {
        assert!(reject_blocked_literal("https://127.0.0.1/secret").is_err());
        assert!(reject_blocked_literal("https://[::1]/secret").is_err());
        assert!(reject_blocked_literal("https://169.254.169.254/latest/meta-data").is_err());
    }

    #[test]
    fn allows_public_literal() {
        assert!(reject_blocked_literal("https://8.8.8.8/").is_ok());
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn live_fetch_example() {
        use goat_tool::Tool;
        let ctx = goat_tool::ToolContext::new(&std::env::temp_dir()).unwrap();
        let out = super::WebFetchTool
            .run(r#"{"url":"https://example.com"}"#, &ctx)
            .await
            .unwrap();
        assert!(out.as_text().unwrap().contains("Example Domain"));
    }
}
