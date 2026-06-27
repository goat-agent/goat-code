use std::fmt::Write as _;
use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::page::StopLoadingParams;
use chromiumoxide::handler::viewport::Viewport;
use chromiumoxide::{Browser, BrowserConfig};
use futures::StreamExt as _;

use goat_auth::{CredentialKey, CredentialStore};
use goat_config::{Config, SearchAccountConfig};
use goat_tool::{Tool, ToolContext, ToolError, ToolFuture, ToolOutput};
use reqwest::Url;
use serde::Deserialize;

const SEARCH_TIMEOUT: Duration = Duration::from_secs(15);
const BROWSER_SEARCH_TIMEOUT: Duration = Duration::from_secs(25);
const DEFAULT_MAX_RESULTS: usize = 8;
const MAX_RESULTS: usize = 20;

pub struct WebSearchTool {
    registry: SearchRegistry,
}

impl WebSearchTool {
    pub fn new() -> Self {
        Self {
            registry: SearchRegistry::load(),
        }
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct Input {
    query: String,
    #[serde(default)]
    max_results: Option<usize>,
    #[serde(default)]
    site: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    time_range: Option<String>,
    #[serde(default)]
    target: Option<String>,
}

impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        "WebSearch"
    }

    fn description(&self) -> &'static str {
        "Search the live web and return ranked URL candidates through an account-aware provider registry. Results are untrusted discovery candidates, not evidence; verify result contents with WebFetch. Built-in providers include browser/duckduckgo ephemeral no-cookie search, duckduckgo/html, configured searxng accounts, and configured API-key providers brave and tavily."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Live web search query." },
                "max_results": { "type": "integer", "description": "Maximum number of ranked candidates to return." },
                "site": { "type": "string", "description": "Optional site/domain restriction." },
                "language": { "type": "string", "description": "Optional language hint." },
                "time_range": { "type": "string", "description": "Optional time range hint. Provider support varies." },
                "target": { "type": "string", "description": "Optional provider/account override such as browser/duckduckgo, duckduckgo/html, searxng/home, brave/work, or tavily/default." }
            },
            "required": ["query"]
        })
    }

    fn run<'a>(&'a self, input: &'a str, _ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: Input = serde_json::from_str(input)?;
            let request = SearchRequest {
                query: args.query,
                max_results: args
                    .max_results
                    .unwrap_or(DEFAULT_MAX_RESULTS)
                    .min(MAX_RESULTS),
                site: args.site,
                language: args.language,
                time_range: args.time_range,
                target: args.target,
            };
            let results =
                self.registry
                    .search(request)
                    .await
                    .map_err(|err| ToolError::Execution {
                        message: err.to_string(),
                    })?;
            Ok(ToolOutput::text(render_results(&results)))
        })
    }
}

#[derive(Debug)]
struct SearchRequest {
    query: String,
    max_results: usize,
    site: Option<String>,
    language: Option<String>,
    time_range: Option<String>,
    target: Option<String>,
}

#[derive(Debug)]
struct SearchResults {
    query: String,
    provider: SearchTarget,
    language: Option<String>,
    time_range: Option<String>,
    limitations: Vec<&'static str>,
    results: Vec<SearchResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchTarget {
    provider: String,
    account: String,
}

impl SearchTarget {
    fn parse(s: &str) -> Option<Self> {
        let (provider, account) = s.split_once('/')?;
        if provider.is_empty() || account.is_empty() {
            return None;
        }
        Some(Self {
            provider: provider.to_owned(),
            account: account.to_owned(),
        })
    }

    fn as_str(&self) -> String {
        format!("{}/{}", self.provider, self.account)
    }
}

#[derive(Debug)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
    rank: usize,
    provider: String,
    published_at: Option<String>,
}

struct SearchRegistry {
    providers: Vec<SearchProvider>,
    default_target: SearchTarget,
    credentials: Option<CredentialStore>,
}

impl SearchRegistry {
    fn load() -> Self {
        let config = Config::load();
        let credentials = goat_config::auth_path().map(CredentialStore::new);
        Self::from_config(config, credentials)
    }

    fn from_config(config: Config, credentials: Option<CredentialStore>) -> Self {
        let mut providers = vec![
            SearchProvider::BrowserDuckDuckGo {
                account: "duckduckgo".to_owned(),
            },
            SearchProvider::DuckDuckGoHtml {
                account: "html".to_owned(),
            },
        ];
        for account in config.search.accounts {
            match account {
                SearchAccountConfig::Duckduckgo { account } => {
                    providers.push(SearchProvider::DuckDuckGoHtml { account });
                }
                SearchAccountConfig::Browser { account, engine } => {
                    if engine == "duckduckgo" {
                        providers.push(SearchProvider::BrowserDuckDuckGo { account });
                    } else {
                        providers.push(SearchProvider::Invalid {
                            target: SearchTarget {
                                provider: "browser".to_owned(),
                                account,
                            },
                            reason: format!("unsupported browser search engine: {engine}"),
                        });
                    }
                }
                SearchAccountConfig::Searxng { account, endpoint } => {
                    providers.push(SearchProvider::Searxng { account, endpoint });
                }
                SearchAccountConfig::Brave { account } => {
                    providers.push(SearchProvider::Brave { account });
                }
                SearchAccountConfig::Tavily { account } => {
                    providers.push(SearchProvider::Tavily { account });
                }
            }
        }
        let default_target = config
            .search
            .default_target
            .as_deref()
            .and_then(SearchTarget::parse)
            .filter(|target| {
                providers
                    .iter()
                    .any(|provider| provider.target() == *target)
            })
            .unwrap_or_else(|| {
                SearchTarget::parse("browser/duckduckgo").expect("valid default target")
            });
        Self {
            providers,
            default_target,
            credentials,
        }
    }

    async fn search(&self, request: SearchRequest) -> Result<SearchResults, SearchError> {
        let target = match request.target.as_deref() {
            Some(raw) => SearchTarget::parse(raw)
                .ok_or_else(|| SearchError::InvalidTarget(raw.to_owned()))?,
            None => self.default_target.clone(),
        };
        let provider = self
            .providers
            .iter()
            .find(|provider| provider.target() == target)
            .ok_or_else(|| SearchError::UnknownTarget(target.as_str()))?;
        provider.search(request, self.credentials.as_ref()).await
    }
}

#[derive(Debug)]
enum SearchProvider {
    BrowserDuckDuckGo {
        account: String,
    },
    DuckDuckGoHtml {
        account: String,
    },
    Searxng {
        account: String,
        endpoint: String,
    },
    Brave {
        account: String,
    },
    Tavily {
        account: String,
    },
    Invalid {
        target: SearchTarget,
        reason: String,
    },
}

impl SearchProvider {
    fn target(&self) -> SearchTarget {
        match self {
            Self::BrowserDuckDuckGo { account } => SearchTarget {
                provider: "browser".to_owned(),
                account: account.clone(),
            },
            Self::DuckDuckGoHtml { account } => SearchTarget {
                provider: "duckduckgo".to_owned(),
                account: account.clone(),
            },
            Self::Searxng { account, .. } => SearchTarget {
                provider: "searxng".to_owned(),
                account: account.clone(),
            },
            Self::Brave { account } => SearchTarget {
                provider: "brave".to_owned(),
                account: account.clone(),
            },
            Self::Tavily { account } => SearchTarget {
                provider: "tavily".to_owned(),
                account: account.clone(),
            },
            Self::Invalid { target, .. } => target.clone(),
        }
    }

    async fn search(
        &self,
        request: SearchRequest,
        credentials: Option<&CredentialStore>,
    ) -> Result<SearchResults, SearchError> {
        match self {
            Self::BrowserDuckDuckGo { .. } => self.search_browser_duckduckgo(request).await,
            Self::DuckDuckGoHtml { .. } => self.search_duckduckgo(request).await,
            Self::Searxng { endpoint, .. } => self.search_searxng(request, endpoint).await,
            Self::Brave { account } => self.search_brave(request, credentials, account).await,
            Self::Tavily { account } => self.search_tavily(request, credentials, account).await,
            Self::Invalid { reason, .. } => Err(SearchError::InvalidProvider(reason.clone())),
        }
    }

    async fn search_browser_duckduckgo(
        &self,
        request: SearchRequest,
    ) -> Result<SearchResults, SearchError> {
        let query = build_query(&request)?;
        let body = fetch_duckduckgo_with_ephemeral_browser(&query).await?;
        let mut results = parse_duckduckgo_html(&body, request.max_results);
        self.fill_provider(&mut results);
        Ok(self.results(
            query,
            request,
            results,
            vec!["ephemeral_no_cookie_browser", "duckduckgo_html_extraction"],
        ))
    }
    async fn search_duckduckgo(
        &self,
        request: SearchRequest,
    ) -> Result<SearchResults, SearchError> {
        let query = build_query(&request)?;
        let mut url = Url::parse("https://html.duckduckgo.com/html/")
            .map_err(|err| SearchError::Url(err.to_string()))?;
        url.query_pairs_mut().append_pair("q", &query);
        let body = client()?
            .get(url)
            .send()
            .await
            .map_err(SearchError::Request)?
            .text()
            .await
            .map_err(SearchError::Request)?;
        let mut results = parse_duckduckgo_html(&body, request.max_results);
        self.fill_provider(&mut results);
        Ok(self.results(query, request, results, vec!["html_result_extraction"]))
    }

    async fn search_searxng(
        &self,
        request: SearchRequest,
        endpoint: &str,
    ) -> Result<SearchResults, SearchError> {
        let query = build_query(&request)?;
        let base = Url::parse(endpoint).map_err(|err| SearchError::Url(err.to_string()))?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(SearchError::Url(format!(
                "unsupported searxng endpoint scheme: {}",
                base.scheme()
            )));
        }
        let mut url = base
            .join("search")
            .map_err(|err| SearchError::Url(err.to_string()))?;
        url.query_pairs_mut()
            .append_pair("q", &query)
            .append_pair("format", "json");
        if let Some(language) = &request.language {
            url.query_pairs_mut().append_pair("language", language);
        }
        if let Some(time_range) = &request.time_range {
            url.query_pairs_mut().append_pair("time_range", time_range);
        }
        let value = client()?
            .get(url)
            .send()
            .await
            .map_err(SearchError::Request)?
            .json::<serde_json::Value>()
            .await
            .map_err(SearchError::Request)?;
        let mut results = parse_searxng_json(&value, request.max_results);
        self.fill_provider(&mut results);
        Ok(self.results(query, request, results, vec!["searxng_json"]))
    }

    async fn search_brave(
        &self,
        request: SearchRequest,
        credentials: Option<&CredentialStore>,
        account: &str,
    ) -> Result<SearchResults, SearchError> {
        let query = build_query(&request)?;
        let token = search_secret(credentials, "brave", account, Some("BRAVE_API_KEY"))?;
        let mut url = Url::parse("https://api.search.brave.com/res/v1/web/search")
            .map_err(|err| SearchError::Url(err.to_string()))?;
        url.query_pairs_mut()
            .append_pair("q", &query)
            .append_pair("count", &request.max_results.to_string());
        if let Some(language) = &request.language {
            url.query_pairs_mut().append_pair("search_lang", language);
        }
        let value = client()?
            .get(url)
            .header("X-Subscription-Token", token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(SearchError::Request)?
            .json::<serde_json::Value>()
            .await
            .map_err(SearchError::Request)?;
        let mut results = parse_brave_json(&value, request.max_results);
        self.fill_provider(&mut results);
        Ok(self.results(query, request, results, vec!["brave_api"]))
    }

    async fn search_tavily(
        &self,
        request: SearchRequest,
        credentials: Option<&CredentialStore>,
        account: &str,
    ) -> Result<SearchResults, SearchError> {
        let query = build_query(&request)?;
        let token = search_secret(credentials, "tavily", account, Some("TAVILY_API_KEY"))?;
        let mut body = serde_json::json!({
            "api_key": token,
            "query": query,
            "max_results": request.max_results,
        });
        if let Some(site) = &request.site {
            body["include_domains"] = serde_json::json!([site]);
        }
        let value = client()?
            .post("https://api.tavily.com/search")
            .json(&body)
            .send()
            .await
            .map_err(SearchError::Request)?
            .json::<serde_json::Value>()
            .await
            .map_err(SearchError::Request)?;
        let mut results = parse_tavily_json(&value, request.max_results);
        self.fill_provider(&mut results);
        Ok(self.results(query, request, results, vec!["tavily_api"]))
    }

    fn fill_provider(&self, results: &mut [SearchResult]) {
        let target = self.target().as_str();
        for result in results {
            result.provider.clone_from(&target);
        }
    }

    fn results(
        &self,
        query: String,
        request: SearchRequest,
        results: Vec<SearchResult>,
        mut limitations: Vec<&'static str>,
    ) -> SearchResults {
        limitations.push("results_are_untrusted_candidates");
        limitations.push("verify_with_webfetch");
        SearchResults {
            query,
            provider: self.target(),
            language: request.language,
            time_range: request.time_range,
            limitations,
            results,
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum SearchError {
    #[error("query is empty")]
    EmptyQuery,
    #[error("invalid search target: {0}")]
    InvalidTarget(String),
    #[error("unknown search target: {0}")]
    UnknownTarget(String),
    #[error("invalid search provider config: {0}")]
    InvalidProvider(String),
    #[error("missing search credential for {0}/{1}")]
    MissingCredential(String, String),
    #[error("browser-backed search failed: {0}")]
    Browser(String),
    #[error("invalid search url: {0}")]
    Url(String),
    #[error("search request failed: {0}")]
    Request(reqwest::Error),
}

fn browser_launch_args() -> [&'static str; 22] {
    [
        "disable-background-networking",
        "enable-features=NetworkService,NetworkServiceInProcess",
        "disable-background-timer-throttling",
        "disable-backgrounding-occluded-windows",
        "disable-breakpad",
        "disable-client-side-phishing-detection",
        "disable-component-extensions-with-background-pages",
        "disable-default-apps",
        "disable-dev-shm-usage",
        "disable-features=TranslateUI",
        "disable-hang-monitor",
        "disable-ipc-flooding-protection",
        "disable-popup-blocking",
        "disable-prompt-on-repost",
        "disable-renderer-backgrounding",
        "disable-sync",
        "force-color-profile=srgb",
        "metrics-recording-only",
        "no-first-run",
        "no-default-browser-check",
        "disable-blink-features=AutomationControlled",
        "incognito",
    ]
}

async fn fetch_duckduckgo_with_ephemeral_browser(query: &str) -> Result<String, SearchError> {
    let profile = tempfile::Builder::new()
        .prefix("goat-websearch-")
        .tempdir()
        .map_err(|err| SearchError::Browser(err.to_string()))?;
    let config = BrowserConfig::builder()
        .new_headless_mode()
        .user_data_dir(profile.path())
        .viewport(None::<Viewport>)
        .launch_timeout(BROWSER_SEARCH_TIMEOUT)
        .request_timeout(BROWSER_SEARCH_TIMEOUT)
        .disable_default_args()
        .args(browser_launch_args())
        .build()
        .map_err(SearchError::Browser)?;
    let (mut browser, mut handler) = Browser::launch(config)
        .await
        .map_err(|err| SearchError::Browser(err.to_string()))?;
    let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });
    let result = async {
        let mut url = Url::parse("https://html.duckduckgo.com/html/")
            .map_err(|err| SearchError::Url(err.to_string()))?;
        url.query_pairs_mut().append_pair("q", query);
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|err| SearchError::Browser(err.to_string()))?;
        match tokio::time::timeout(BROWSER_SEARCH_TIMEOUT, page.goto(url.to_string())).await {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => return Err(SearchError::Browser(err.to_string())),
            Err(_) => {
                let _ = page.execute(StopLoadingParams::default()).await;
            }
        }
        page.content()
            .await
            .map_err(|err| SearchError::Browser(err.to_string()))
    }
    .await;
    let _ = browser.close().await;
    handler_task.abort();
    result
}
fn client() -> Result<reqwest::Client, SearchError> {
    reqwest::Client::builder()
        .timeout(SEARCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(4))
        .user_agent("goat-code WebSearch")
        .build()
        .map_err(SearchError::Request)
}

fn search_secret(
    credentials: Option<&CredentialStore>,
    provider: &str,
    account: &str,
    env_var: Option<&str>,
) -> Result<String, SearchError> {
    let key = CredentialKey::search(provider, account);
    credentials
        .and_then(|store| store.resolve(&key, env_var))
        .map(|credential| credential.bearer().to_owned())
        .ok_or_else(|| SearchError::MissingCredential(provider.to_owned(), account.to_owned()))
}

fn build_query(request: &SearchRequest) -> Result<String, SearchError> {
    let mut query = request.query.trim().to_owned();
    if query.is_empty() {
        return Err(SearchError::EmptyQuery);
    }
    if let Some(site) = &request.site
        && !site.trim().is_empty()
    {
        query = format!("site:{} {query}", site.trim());
    }
    Ok(query)
}

fn render_results(results: &SearchResults) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "query: {}", results.query);
    let _ = writeln!(out, "target: {}", results.provider.as_str());
    out.push_str("source_trust: untrusted_web\n");
    out.push_str("result_role: discovery_candidates_not_evidence\n");
    if let Some(language) = &results.language {
        let _ = writeln!(out, "language: {language}");
    }
    if let Some(time_range) = &results.time_range {
        let _ = writeln!(out, "time_range: {time_range}");
    }
    out.push_str("limitations:\n");
    for limitation in &results.limitations {
        let _ = writeln!(out, "- {limitation}");
    }
    out.push_str("results:\n");
    if results.results.is_empty() {
        out.push_str("- none\n");
    } else {
        for result in &results.results {
            let _ = writeln!(out, "- rank: {}", result.rank);
            let _ = writeln!(out, "  title: {}", result.title);
            let _ = writeln!(out, "  url: {}", result.url);
            let _ = writeln!(out, "  snippet: {}", result.snippet);
            let _ = writeln!(out, "  provider: {}", result.provider);
            if let Some(published_at) = &result.published_at {
                let _ = writeln!(out, "  published_at: {published_at}");
            }
        }
    }
    out.push_str("next_step: use WebFetch on candidate URLs before treating content as evidence\n");
    out
}

fn parse_duckduckgo_html(html: &str, max_results: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();
    let mut index = 0usize;
    while results.len() < max_results {
        let Some(link_rel) = html[index..].find("result__a") else {
            break;
        };
        let link_pos = index + link_rel;
        let Some(tag_start_rel) = html[..link_pos].rfind("<a") else {
            index = link_pos + "result__a".len();
            continue;
        };
        let Some(tag_end_rel) = html[tag_start_rel..].find('>') else {
            break;
        };
        let tag_end = tag_start_rel + tag_end_rel + 1;
        let tag = &html[tag_start_rel..tag_end];
        let Some(raw_href) = attr_value(tag, "href") else {
            index = tag_end;
            continue;
        };
        let Some(end_rel) = html[tag_end..].find("</a>") else {
            break;
        };
        let end = tag_end + end_rel;
        let title = clean_text(&strip_tags(&html[tag_end..end]));
        let url = normalize_duckduckgo_url(&raw_href);
        if title.is_empty() || url.is_empty() {
            index = end + 4;
            continue;
        }
        let snippet = find_snippet(&html[end..]).unwrap_or_default();
        let rank = results.len() + 1;
        results.push(SearchResult {
            title,
            url,
            snippet,
            rank,
            provider: String::new(),
            published_at: None,
        });
        index = end + 4;
    }
    results
}

fn parse_searxng_json(value: &serde_json::Value, max_results: usize) -> Vec<SearchResult> {
    value
        .get("results")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .take(max_results)
        .enumerate()
        .filter_map(|(index, item)| {
            let title = item.get("title").and_then(serde_json::Value::as_str)?;
            let url = item.get("url").and_then(serde_json::Value::as_str)?;
            let snippet = item
                .get("content")
                .or_else(|| item.get("snippet"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            Some(SearchResult {
                title: clean_text(title),
                url: url.to_owned(),
                snippet: clean_text(snippet),
                rank: index + 1,
                provider: String::new(),
                published_at: item
                    .get("publishedDate")
                    .or_else(|| item.get("published_at"))
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
            })
        })
        .collect()
}

fn parse_brave_json(value: &serde_json::Value, max_results: usize) -> Vec<SearchResult> {
    value
        .get("web")
        .and_then(|web| web.get("results"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .take(max_results)
        .enumerate()
        .filter_map(|(index, item)| {
            let title = item.get("title").and_then(serde_json::Value::as_str)?;
            let url = item.get("url").and_then(serde_json::Value::as_str)?;
            let snippet = item
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            Some(SearchResult {
                title: clean_text(&strip_tags(title)),
                url: url.to_owned(),
                snippet: clean_text(&strip_tags(snippet)),
                rank: index + 1,
                provider: String::new(),
                published_at: item
                    .get("age")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
            })
        })
        .collect()
}

fn parse_tavily_json(value: &serde_json::Value, max_results: usize) -> Vec<SearchResult> {
    value
        .get("results")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .take(max_results)
        .enumerate()
        .filter_map(|(index, item)| {
            let title = item.get("title").and_then(serde_json::Value::as_str)?;
            let url = item.get("url").and_then(serde_json::Value::as_str)?;
            let snippet = item
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            Some(SearchResult {
                title: clean_text(title),
                url: url.to_owned(),
                snippet: clean_text(snippet),
                rank: index + 1,
                provider: String::new(),
                published_at: item
                    .get("published_date")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
            })
        })
        .collect()
}

fn find_snippet(after: &str) -> Option<String> {
    let pos = after.find("result__snippet")?;
    let open = after[pos..].find('>')? + pos + 1;
    let close = after[open..]
        .find("</a>")
        .or_else(|| after[open..].find("</div>"))?
        + open;
    let snippet = clean_text(&strip_tags(&after[open..close]));
    (!snippet.is_empty()).then_some(snippet)
}

fn normalize_duckduckgo_url(raw: &str) -> String {
    let decoded = decode_entities(raw);
    let normalized = if decoded.starts_with("//") {
        format!("https:{decoded}")
    } else {
        decoded
    };
    if let Ok(url) = Url::parse(&normalized) {
        if url.domain() == Some("duckduckgo.com")
            && url.path() == "/l/"
            && let Some(target) = url
                .query_pairs()
                .find_map(|(key, value)| (key == "uddg").then_some(value))
        {
            return target.into_owned();
        }
        return url.to_string();
    }
    normalized
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

#[cfg(test)]
mod tests {
    use super::{
        SearchRegistry, SearchTarget, parse_brave_json, parse_duckduckgo_html, parse_searxng_json,
        parse_tavily_json, render_results,
    };

    #[test]
    fn parses_target() {
        let target = SearchTarget::parse("duckduckgo/html").unwrap();
        assert_eq!(target.provider, "duckduckgo");
        assert_eq!(target.account, "html");
        assert!(SearchTarget::parse("duckduckgo").is_none());
    }

    #[test]
    fn registry_loads_configured_accounts() {
        let cfg: goat_config::Config = serde_json::from_str(
            r#"{
                "search": {
                    "default_target": "searxng/home",
                    "accounts": [
                        { "provider": "searxng", "account": "home", "endpoint": "https://search.example.com" },
                        { "provider": "brave", "account": "work" },
                        { "provider": "tavily", "account": "research" }
                    ]
                }
            }"#,
        )
        .unwrap();
        let registry = SearchRegistry::from_config(cfg, None);
        assert_eq!(registry.default_target.as_str(), "searxng/home");
        let targets: Vec<String> = registry
            .providers
            .iter()
            .map(|provider| provider.target().as_str())
            .collect();
        assert!(targets.contains(&"browser/duckduckgo".to_owned()));
        assert!(targets.contains(&"duckduckgo/html".to_owned()));
        assert!(targets.contains(&"searxng/home".to_owned()));
        assert!(targets.contains(&"brave/work".to_owned()));
        assert!(targets.contains(&"tavily/research".to_owned()));
    }

    #[test]
    fn parses_duckduckgo_results() {
        let html = r#"
            <div class="result">
              <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa&amp;rut=x">Example A</a>
              <a class="result__snippet">Snippet <b>A</b></a>
            </div>
            <div class="result">
              <a class='result__a' href="https://example.org/b">Example B</a>
              <div class="result__snippet">Snippet B</div>
            </div>
        "#;
        let results = parse_duckduckgo_html(html, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Example A");
        assert_eq!(results[0].url, "https://example.com/a");
        assert_eq!(results[0].snippet, "Snippet A");
        assert_eq!(results[1].url, "https://example.org/b");
    }

    #[test]
    fn parses_provider_json_results() {
        let searxng = serde_json::json!({
            "results": [{ "title": "A", "url": "https://a.test", "content": "Alpha" }]
        });
        assert_eq!(parse_searxng_json(&searxng, 10)[0].url, "https://a.test");
        let brave = serde_json::json!({
            "web": { "results": [{ "title": "<b>B</b>", "url": "https://b.test", "description": "Bravo" }] }
        });
        assert_eq!(parse_brave_json(&brave, 10)[0].title, "B");
        let tavily = serde_json::json!({
            "results": [{ "title": "C", "url": "https://c.test", "content": "Charlie" }]
        });
        assert_eq!(parse_tavily_json(&tavily, 10)[0].snippet, "Charlie");
    }

    #[test]
    fn renders_candidates_as_untrusted() {
        let results = super::SearchResults {
            query: "goat".to_owned(),
            provider: SearchTarget::parse("duckduckgo/html").unwrap(),
            language: None,
            time_range: None,
            limitations: vec!["verify_with_webfetch"],
            results: vec![super::SearchResult {
                title: "Goat".to_owned(),
                url: "https://example.com".to_owned(),
                snippet: "A result".to_owned(),
                rank: 1,
                provider: "duckduckgo/html".to_owned(),
                published_at: None,
            }],
        };
        let out = render_results(&results);
        assert!(out.contains("source_trust: untrusted_web"));
        assert!(out.contains("result_role: discovery_candidates_not_evidence"));
        assert!(out.contains("next_step: use WebFetch"));
    }
}
