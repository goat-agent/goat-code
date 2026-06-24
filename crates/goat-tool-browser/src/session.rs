use std::collections::HashSet;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chromiumoxide::cdp::browser_protocol::browser::BrowserContextId;
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::cdp::browser_protocol::target::CreateTargetParams;
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use chromiumoxide::error::CdpError;
use chromiumoxide::handler::viewport::Viewport;
use chromiumoxide::page::ScreenshotParams;
use chromiumoxide::{Browser, BrowserConfig, Element, Page};
use futures::StreamExt as _;
use goat_tool::{ToolImage, ToolOutput};
use tokio::sync::{Mutex, OnceCell};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::action::Action;
use crate::error::BrowserError;
use crate::snapshot::{RawSnapshot, SNAPSHOT_JS, format_snapshot};

const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);
const CMD_TIMEOUT: Duration = Duration::from_secs(30);
const NAV_TIMEOUT: Duration = Duration::from_secs(30);
const SETTLE_TIMEOUT: Duration = Duration::from_secs(2);
const SNAPSHOT_MAX_BYTES: usize = 32 * 1024;
const SCREENSHOT_MAX_DIM: u32 = 1280;
const HANDLER_MAX_CONSECUTIVE_ERRORS: u32 = 64;

const LAUNCH_ARGS: [&str; 21] = [
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
];

pub type SessionHandle = Arc<Mutex<Option<BrowserSession>>>;

pub fn new_handle() -> SessionHandle {
    Arc::new(Mutex::new(None))
}

struct SharedBrowser {
    browser: Browser,
    handler_task: JoinHandle<()>,
}

static SHARED: OnceCell<Mutex<Option<Arc<SharedBrowser>>>> = OnceCell::const_new();

async fn shared_cell() -> &'static Mutex<Option<Arc<SharedBrowser>>> {
    SHARED.get_or_init(|| async { Mutex::new(None) }).await
}

async fn shared_browser() -> Result<Arc<SharedBrowser>, BrowserError> {
    let cell = shared_cell().await;
    let mut guard = cell.lock().await;
    if let Some(shared) = guard.as_ref()
        && !shared.handler_task.is_finished()
    {
        return Ok(shared.clone());
    }
    if let Some(old) = guard.take() {
        old.handler_task.abort();
    }
    let shared = Arc::new(launch_shared().await?);
    *guard = Some(shared.clone());
    Ok(shared)
}

async fn launch_shared() -> Result<SharedBrowser, BrowserError> {
    match launch_shared_once().await {
        Ok(shared) => Ok(shared),
        Err(first) => {
            clear_singleton_locks();
            launch_shared_once().await.map_err(|_| first)
        }
    }
}

fn clear_singleton_locks() {
    let Some(profile) = goat_config::browser_profile_dir() else {
        return;
    };
    for name in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
        let _ = std::fs::remove_file(profile.join(name));
    }
}

async fn launch_shared_once() -> Result<SharedBrowser, BrowserError> {
    let profile = goat_config::browser_profile_dir().ok_or(BrowserError::NoProfile)?;
    std::fs::create_dir_all(&profile).map_err(|err| {
        BrowserError::Message(format!("could not create browser profile dir: {err}"))
    })?;

    let config = BrowserConfig::builder()
        .with_head()
        .user_data_dir(profile)
        .viewport(None::<Viewport>)
        .launch_timeout(LAUNCH_TIMEOUT)
        .request_timeout(CMD_TIMEOUT)
        .disable_default_args()
        .surface_invalid_messages()
        .args(LAUNCH_ARGS)
        .build()
        .map_err(BrowserError::NoChrome)?;

    let built = tokio::spawn(async move {
        let (browser, mut handler) = Browser::launch(config).await?;
        let handler_task = tokio::spawn(async move {
            let mut consecutive_errors: u32 = 0;
            while let Some(event) = handler.next().await {
                if let Err(err) = event {
                    consecutive_errors += 1;
                    tracing::debug!(%err, consecutive_errors, "browser handler event error");
                    if handler_should_stop(consecutive_errors) {
                        tracing::warn!(
                            consecutive_errors,
                            "browser handler stopping after sustained errors; chrome will relaunch on next use"
                        );
                        break;
                    }
                } else {
                    consecutive_errors = 0;
                }
            }
        });
        Ok::<_, CdpError>(SharedBrowser {
            browser,
            handler_task,
        })
    })
    .await
    .map_err(|err| BrowserError::Message(format!("browser launch task failed: {err}")))?;

    built.map_err(map_launch_err)
}

fn handler_should_stop(consecutive_errors: u32) -> bool {
    consecutive_errors >= HANDLER_MAX_CONSECUTIVE_ERRORS
}

pub struct BrowserSession {
    shared: Arc<SharedBrowser>,
    context_id: BrowserContextId,
    page: Page,
    known_targets: HashSet<String>,
}

pub async fn ensure_session(
    slot: &mut Option<BrowserSession>,
) -> Result<&mut BrowserSession, BrowserError> {
    let alive = matches!(slot, Some(session) if !session.shared.handler_task.is_finished());
    if !alive {
        if let Some(old) = slot.take() {
            old.dispose().await;
        }
        *slot = Some(open_session().await?);
    }
    slot.as_mut()
        .ok_or_else(|| BrowserError::Message("browser session unavailable".to_owned()))
}

async fn open_session() -> Result<BrowserSession, BrowserError> {
    let shared = shared_browser().await?;
    let context_id = shared
        .browser
        .create_browser_context(
            chromiumoxide::cdp::browser_protocol::target::CreateBrowserContextParams::default(),
        )
        .await
        .map_err(map_launch_err)?;
    let params = CreateTargetParams::builder()
        .url("about:blank")
        .browser_context_id(context_id.clone())
        .build()
        .map_err(BrowserError::Message)?;
    let page = shared
        .browser
        .new_page(params)
        .await
        .map_err(map_launch_err)?;
    let _ = page.bring_to_front().await;
    let mut known_targets = HashSet::new();
    known_targets.insert(page.target_id().inner().clone());
    Ok(BrowserSession {
        shared,
        context_id,
        page,
        known_targets,
    })
}

fn map_launch_err(err: CdpError) -> BrowserError {
    match err {
        CdpError::LaunchExit(..) => BrowserError::Message(
            "Chrome exited during launch; the profile at ~/.goat-code/browser/profile may be locked by another goat-code instance".to_owned(),
        ),
        CdpError::LaunchTimeout(_) => {
            BrowserError::Message("Chrome did not start within the launch timeout".to_owned())
        }
        other => BrowserError::Message(format!("could not launch Chrome: {other}")),
    }
}

pub async fn close(slot: &mut Option<BrowserSession>) -> String {
    let Some(session) = slot.take() else {
        return "browser is not running".to_owned();
    };
    session.dispose().await;
    "browser closed".to_owned()
}

impl BrowserSession {
    async fn dispose(self) {
        let _ = self
            .shared
            .browser
            .dispose_browser_context(self.context_id)
            .await;
    }

    pub async fn dispatch(
        &mut self,
        action: Action,
        max_bytes: usize,
    ) -> Result<ToolOutput, BrowserError> {
        let output = match action {
            Action::Navigate { url } => ToolOutput::text(self.navigate(&url).await?),
            Action::Snapshot => ToolOutput::text(run_snapshot(&self.page).await?),
            Action::Click { reference } => ToolOutput::text(self.click(&reference).await?),
            Action::Type {
                reference,
                text,
                submit,
            } => ToolOutput::text(self.type_text(&reference, &text, submit).await?),
            Action::Select { reference, value } => {
                ToolOutput::text(self.select(&reference, &value).await?)
            }
            Action::PressKey { key } => ToolOutput::text(self.press_key(&key).await?),
            Action::Evaluate { js } => ToolOutput::text(self.evaluate(&js, max_bytes).await?),
            Action::Screenshot => ToolOutput::image(self.screenshot().await?),
            Action::Close => ToolOutput::text("browser closed".to_owned()),
        };
        Ok(output)
    }

    async fn navigate(&mut self, url: &str) -> Result<String, BrowserError> {
        let target = normalize_url(url)?;
        let started = timeout(NAV_TIMEOUT, self.page.goto(target)).await;
        match started {
            Ok(result) => {
                result?;
            }
            Err(_) => {
                return Err(BrowserError::Message(
                    "navigation timed out; the page may still be loading, try the snapshot action"
                        .to_owned(),
                ));
            }
        }
        self.settle_and_snapshot().await
    }

    async fn click(&mut self, reference: &str) -> Result<String, BrowserError> {
        let element = self.find_ref(reference).await?;
        let _ = element.scroll_into_view().await;
        element.click().await?;
        self.settle_and_snapshot().await
    }

    async fn type_text(
        &mut self,
        reference: &str,
        text: &str,
        submit: bool,
    ) -> Result<String, BrowserError> {
        let element = self.find_ref(reference).await?;
        let _ = element.scroll_into_view().await;
        element.click().await?;
        let _ = element
            .call_js_fn(
                "function() { if ('value' in this) { this.value = ''; this.dispatchEvent(new Event('input', { bubbles: true })); } }",
                false,
            )
            .await;
        element.type_str(text).await?;
        if submit {
            element.press_key("Enter").await?;
        }
        self.settle_and_snapshot().await
    }

    async fn select(&mut self, reference: &str, value: &str) -> Result<String, BrowserError> {
        let element = self.find_ref(reference).await?;
        let literal =
            serde_json::to_string(value).map_err(|err| BrowserError::Message(err.to_string()))?;
        let declaration = format!(
            "function() {{ const v = {literal}; for (let i = 0; i < this.options.length; i++) {{ const o = this.options[i]; if (o.value === v || o.text.trim() === v) {{ this.selectedIndex = i; this.dispatchEvent(new Event('input', {{ bubbles: true }})); this.dispatchEvent(new Event('change', {{ bubbles: true }})); return true; }} }} return false; }}"
        );
        let returns = element.call_js_fn(declaration, false).await?;
        let matched = returns
            .result
            .value
            .as_ref()
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if !matched {
            return Err(BrowserError::Input(format!(
                "no option matching \"{value}\" in {reference}"
            )));
        }
        self.settle_and_snapshot().await
    }

    async fn press_key(&mut self, key: &str) -> Result<String, BrowserError> {
        let element = match self.page.find_element(":focus").await {
            Ok(element) => element,
            Err(_) => self.page.find_element("body").await?,
        };
        element.press_key(key).await?;
        self.settle_and_snapshot().await
    }

    async fn evaluate(&self, js: &str, max_bytes: usize) -> Result<String, BrowserError> {
        let result = self.page.evaluate(js).await?;
        let rendered = match result.value() {
            Some(value) => {
                serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
            }
            None => "undefined".to_owned(),
        };
        Ok(goat_tool::truncate(rendered, max_bytes))
    }

    async fn screenshot(&self) -> Result<ToolImage, BrowserError> {
        let params = ScreenshotParams::builder()
            .format(CaptureScreenshotFormat::Png)
            .full_page(false)
            .build();
        let bytes = self.page.screenshot(params).await?;
        let encoded = downscale_png(&bytes).unwrap_or(bytes);
        Ok(ToolImage {
            media_type: "image/png".to_owned(),
            data: BASE64.encode(&encoded),
        })
    }

    async fn find_ref(&self, reference: &str) -> Result<Element, BrowserError> {
        let selector = format!("[data-goat-ref='{reference}']");
        self.page.find_element(selector).await.map_err(|_| {
            BrowserError::Input(format!(
                "ref {reference} not found; the page changed - take a new snapshot"
            ))
        })
    }

    async fn settle_and_snapshot(&mut self) -> Result<String, BrowserError> {
        let _ = timeout(SETTLE_TIMEOUT, self.page.wait_for_navigation()).await;
        let switched = self.follow_new_tab().await;
        let _ = self.page.bring_to_front().await;
        let snapshot = run_snapshot(&self.page).await?;
        if switched {
            Ok(format!("[switched to newly opened tab]\n\n{snapshot}"))
        } else {
            Ok(snapshot)
        }
    }

    async fn follow_new_tab(&mut self) -> bool {
        let Ok(pages) = self.shared.browser.pages().await else {
            return false;
        };
        let mut newest: Option<Page> = None;
        for page in pages {
            let target = page.target_id().inner().clone();
            if self.known_targets.insert(target) {
                newest = Some(page);
            }
        }
        if let Some(page) = newest {
            self.page = page;
            return true;
        }
        false
    }
}

async fn run_snapshot(page: &Page) -> Result<String, BrowserError> {
    let params = EvaluateParams::builder()
        .expression(SNAPSHOT_JS)
        .return_by_value(true)
        .build()
        .map_err(|err| BrowserError::Message(format!("snapshot eval build: {err}")))?;
    let result = page.evaluate(params).await?;
    let raw: RawSnapshot = result
        .into_value()
        .map_err(|err| BrowserError::Message(format!("could not parse snapshot: {err}")))?;
    let url = page.url().await.ok().flatten().unwrap_or_default();
    Ok(format_snapshot(&url, &raw, SNAPSHOT_MAX_BYTES))
}

fn normalize_url(url: &str) -> Result<String, BrowserError> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(BrowserError::Input("url is empty".to_owned()));
    }
    for scheme in ["http://", "https://", "about:", "file://", "data:"] {
        if trimmed.starts_with(scheme) {
            return Ok(trimmed.to_owned());
        }
    }
    if trimmed.contains("://") {
        return Err(BrowserError::Input(format!(
            "unsupported url scheme in '{trimmed}'"
        )));
    }
    Ok(format!("https://{trimmed}"))
}

fn downscale_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory(bytes).ok()?;
    if image.width() <= SCREENSHOT_MAX_DIM && image.height() <= SCREENSHOT_MAX_DIM {
        return None;
    }
    let scaled = image.resize(
        SCREENSHOT_MAX_DIM,
        SCREENSHOT_MAX_DIM,
        image::imageops::FilterType::Triangle,
    );
    let mut buffer = Cursor::new(Vec::new());
    scaled.write_to(&mut buffer, image::ImageFormat::Png).ok()?;
    Some(buffer.into_inner())
}

#[cfg(test)]
mod tests {
    use super::{HANDLER_MAX_CONSECUTIVE_ERRORS, handler_should_stop, normalize_url};

    #[test]
    fn handler_keeps_running_below_threshold() {
        assert!(!handler_should_stop(0));
        assert!(!handler_should_stop(1));
        assert!(!handler_should_stop(HANDLER_MAX_CONSECUTIVE_ERRORS - 1));
    }

    #[test]
    fn handler_stops_at_threshold() {
        assert!(handler_should_stop(HANDLER_MAX_CONSECUTIVE_ERRORS));
        assert!(handler_should_stop(HANDLER_MAX_CONSECUTIVE_ERRORS + 1));
    }

    #[test]
    fn adds_https_scheme() {
        assert_eq!(normalize_url("example.com").unwrap(), "https://example.com");
    }

    #[test]
    fn preserves_known_schemes() {
        assert_eq!(normalize_url("http://x.com/a").unwrap(), "http://x.com/a");
        assert_eq!(normalize_url("about:blank").unwrap(), "about:blank");
    }

    #[test]
    fn rejects_unknown_scheme() {
        assert!(normalize_url("ftp://x.com").is_err());
    }

    #[test]
    fn rejects_empty() {
        assert!(normalize_url("   ").is_err());
    }
}
