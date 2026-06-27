use std::fmt::Write as _;
use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chromiumoxide::cdp::browser_protocol::page::{CaptureScreenshotFormat, StopLoadingParams};
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use chromiumoxide::error::CdpError;
use chromiumoxide::handler::viewport::Viewport;
use chromiumoxide::page::ScreenshotParams;
use chromiumoxide::{Browser, BrowserConfig, Element, Page};
use futures::StreamExt as _;
use goat_tool::{ToolImage, ToolOutput};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

use crate::action::{Action, BrowserRef, ScrollDirection};
use crate::error::BrowserError;
use crate::snapshot::{BrowserSnapshot, RawSnapshot, SNAPSHOT_JS, format_snapshot};

const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);
const CMD_TIMEOUT: Duration = Duration::from_secs(30);
const NAV_TIMEOUT: Duration = Duration::from_secs(30);
const SETTLE_TIMEOUT: Duration = Duration::from_secs(2);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
const SNAPSHOT_MAX_BYTES: usize = 32 * 1024;
const SCREENSHOT_MAX_DIM: u32 = 1280;
const DEFAULT_TEXT_MAX_BYTES: usize = 8 * 1024;

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

pub struct BrowserSession {
    browser: Browser,
    handler_task: JoinHandle<()>,
    page: Page,
    page_count: usize,
    snapshot_seq: u64,
    current_snapshot_id: Option<String>,
}

impl Drop for BrowserSession {
    fn drop(&mut self) {
        self.handler_task.abort();
    }
}

pub async fn ensure_session(
    slot: &mut Option<BrowserSession>,
) -> Result<&mut BrowserSession, BrowserError> {
    let alive = matches!(slot, Some(session) if !session.handler_task.is_finished());
    if !alive {
        if let Some(mut old) = slot.take() {
            let _ = old.browser.kill().await;
        }
        *slot = Some(launch().await?);
    }
    Ok(slot.as_mut().expect("session was set above"))
}

async fn launch() -> Result<BrowserSession, BrowserError> {
    match launch_once().await {
        Ok(session) => Ok(session),
        Err(first) => {
            clear_singleton_locks();
            launch_once().await.map_err(|_| first)
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

async fn launch_once() -> Result<BrowserSession, BrowserError> {
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
        .args(LAUNCH_ARGS)
        .build()
        .map_err(BrowserError::NoChrome)?;

    let built = tokio::spawn(async move {
        let (browser, mut handler) = Browser::launch(config).await?;
        let handler_task = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(err) = event {
                    tracing::debug!(%err, "browser handler event error");
                }
            }
        });
        let page = browser.new_page("about:blank").await?;
        if let Ok(others) = browser.pages().await {
            for other in others {
                if other.target_id() != page.target_id() {
                    let _ = other.close().await;
                }
            }
        }
        let _ = page.bring_to_front().await;
        Ok::<_, CdpError>(BrowserSession {
            browser,
            handler_task,
            page,
            page_count: 1,
            snapshot_seq: 0,
            current_snapshot_id: None,
        })
    })
    .await
    .map_err(|err| BrowserError::Message(format!("browser launch task failed: {err}")))?;

    built.map_err(map_launch_err)
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
    let Some(mut session) = slot.take() else {
        return "browser is not running".to_owned();
    };
    let graceful = timeout(CLOSE_TIMEOUT, session.browser.close()).await;
    if matches!(graceful, Ok(Ok(_))) {
        let _ = timeout(CLOSE_TIMEOUT, session.browser.wait()).await;
    } else {
        let _ = session.browser.kill().await;
    }
    "browser closed".to_owned()
}

impl BrowserSession {
    pub async fn dispatch(
        &mut self,
        action: Action,
        max_bytes: usize,
    ) -> Result<ToolOutput, BrowserError> {
        self.page_count = self
            .browser
            .pages()
            .await
            .map_or(self.page_count, |pages| pages.len());
        let output = match action {
            Action::Navigate { url } => ToolOutput::text(self.navigate(&url, max_bytes).await?),
            Action::Snapshot => ToolOutput::text(
                self.snapshot("snapshot -> complete", "complete", false, max_bytes)
                    .await?,
            ),
            Action::Click { reference } => {
                ToolOutput::text(self.click(&reference, max_bytes).await?)
            }
            Action::Fill {
                reference,
                text,
                submit,
            } => ToolOutput::text(self.fill(&reference, &text, submit, max_bytes).await?),
            Action::Select { reference, value } => {
                ToolOutput::text(self.select(&reference, &value, max_bytes).await?)
            }
            Action::PressKey { key } => ToolOutput::text(self.press_key(&key, max_bytes).await?),
            Action::Scroll { direction, amount } => {
                ToolOutput::text(self.scroll(direction, amount, max_bytes).await?)
            }
            Action::GoBack => ToolOutput::text(self.history(-1, max_bytes).await?),
            Action::GoForward => ToolOutput::text(self.history(1, max_bytes).await?),
            Action::FindText { query, max_chars } => ToolOutput::text(
                self.find_text(
                    &query,
                    max_chars.unwrap_or(DEFAULT_TEXT_MAX_BYTES),
                    max_bytes,
                )
                .await?,
            ),
            Action::Inspect {
                reference,
                max_chars,
            } => ToolOutput::text(
                self.inspect(
                    &reference,
                    max_chars.unwrap_or(DEFAULT_TEXT_MAX_BYTES),
                    max_bytes,
                )
                .await?,
            ),
            Action::ReadViewport { max_chars } => ToolOutput::text(
                self.read_viewport(max_chars.unwrap_or(DEFAULT_TEXT_MAX_BYTES), max_bytes)
                    .await?,
            ),
            Action::WaitFor {
                text,
                state,
                timeout_ms,
            } => ToolOutput::text(
                self.wait_for(text.as_deref(), state.as_deref(), timeout_ms, max_bytes)
                    .await?,
            ),
            Action::Screenshot => ToolOutput::Image(self.screenshot().await?),
            Action::DebugEval { js } => ToolOutput::text(self.debug_eval(&js, max_bytes).await?),
            Action::Close => ToolOutput::text("browser closed".to_owned()),
        };
        Ok(output)
    }

    async fn navigate(&mut self, url: &str, max_bytes: usize) -> Result<String, BrowserError> {
        let target = normalize_url(url)?;
        let started = timeout(NAV_TIMEOUT, self.page.goto(target)).await;
        if let Ok(result) = started {
            result?;
            self.settle_and_snapshot("navigate -> usable", "complete", max_bytes)
                .await
        } else {
            let _ = self.page.execute(StopLoadingParams::default()).await;
            self.snapshot(
                "navigate -> usable_stopped_loading",
                "partial_stopped",
                false,
                max_bytes,
            )
            .await
        }
    }

    async fn click(
        &mut self,
        reference: &BrowserRef,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        self.validate_snapshot(reference)?;
        let element = self.find_ref(&reference.reference).await?;
        ensure_actionable(&element, "click").await?;
        let _ = element.scroll_into_view().await;
        element.click().await?;
        self.settle_and_snapshot("click -> changed", "complete", max_bytes)
            .await
    }

    async fn fill(
        &mut self,
        reference: &BrowserRef,
        text: &str,
        submit: bool,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        self.validate_snapshot(reference)?;
        let element = self.find_ref(&reference.reference).await?;
        ensure_actionable(&element, "fill").await?;
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
        self.settle_and_snapshot("fill -> changed", "complete", max_bytes)
            .await
    }

    async fn select(
        &mut self,
        reference: &BrowserRef,
        value: &str,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        self.validate_snapshot(reference)?;
        let element = self.find_ref(&reference.reference).await?;
        ensure_actionable(&element, "select").await?;
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
                "no option matching \"{value}\" in {}",
                reference.reference
            )));
        }
        self.settle_and_snapshot("select -> changed", "complete", max_bytes)
            .await
    }

    async fn press_key(&mut self, key: &str, max_bytes: usize) -> Result<String, BrowserError> {
        let element = match self.page.find_element(":focus").await {
            Ok(element) => element,
            Err(_) => self.page.find_element("body").await?,
        };
        element.press_key(key).await?;
        self.settle_and_snapshot("press_key -> changed", "complete", max_bytes)
            .await
    }

    async fn scroll(
        &mut self,
        direction: ScrollDirection,
        amount: Option<i64>,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        let amount = amount.unwrap_or(640).abs();
        let (x, y) = match direction {
            ScrollDirection::Up => (0, -amount),
            ScrollDirection::Down => (0, amount),
            ScrollDirection::Left => (-amount, 0),
            ScrollDirection::Right => (amount, 0),
        };
        let js = format!("window.scrollBy({{ left: {x}, top: {y}, behavior: 'instant' }}); true");
        let _ = self.page.evaluate(js.as_str()).await?;
        self.snapshot("scroll -> changed", "complete", false, max_bytes)
            .await
    }

    async fn history(&mut self, delta: i32, max_bytes: usize) -> Result<String, BrowserError> {
        let js = format!("history.go({delta}); true");
        let _ = self.page.evaluate(js.as_str()).await?;
        let action = if delta < 0 {
            "go_back -> navigation"
        } else {
            "go_forward -> navigation"
        };
        self.settle_and_snapshot(action, "complete", max_bytes)
            .await
    }

    async fn find_text(
        &mut self,
        query: &str,
        max_chars: usize,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        let literal =
            serde_json::to_string(query).map_err(|err| BrowserError::Message(err.to_string()))?;
        let js = format!(
            "(() => {{ const q = {literal}.toLowerCase(); const walker = document.createTreeWalker(document.body || document.documentElement, NodeFilter.SHOW_TEXT); const out = []; while (walker.nextNode() && out.length < 20) {{ const t = walker.currentNode.textContent.trim().replace(/\\s+/g, ' '); if (t.toLowerCase().includes(q)) out.push(t.slice(0, 240)); }} return out; }})()"
        );
        let result = self.page.evaluate(js.as_str()).await?;
        let mut out = self
            .state_header("find_text -> complete", "complete", max_bytes)
            .await?;
        out.push_str("\nuntrusted_text_matches:\n");
        if let Some(items) = result.value().and_then(serde_json::Value::as_array) {
            if items.is_empty() {
                out.push_str("- none\n");
            } else {
                for item in items {
                    if let Some(text) = item.as_str() {
                        let _ = writeln!(out, "- \"{}\"", cap_chars(text, max_chars.min(240)));
                    }
                }
            }
        } else {
            out.push_str("- none\n");
        }
        Ok(cap(out, max_bytes))
    }

    async fn inspect(
        &mut self,
        reference: &BrowserRef,
        max_chars: usize,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        self.validate_snapshot(reference)?;
        let element = self.find_ref(&reference.reference).await?;
        let returns = element
            .call_js_fn(
                "function() { return { role: this.getAttribute('role') || this.tagName.toLowerCase(), text: (this.innerText || this.value || '').trim().replace(/\\s+/g, ' ').slice(0, 4000), disabled: !!this.disabled, readonly: !!this.readOnly }; }",
                false,
            )
            .await?;
        let mut out = self
            .state_header("inspect -> complete", "complete", max_bytes)
            .await?;
        out.push_str("\nuntrusted_region:\n");
        if let Some(value) = returns.result.value {
            let rendered =
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
            out.push_str(&cap_chars(&rendered, max_chars));
            out.push('\n');
        } else {
            out.push_str("- none\n");
        }
        Ok(cap(out, max_bytes))
    }

    async fn read_viewport(
        &mut self,
        max_chars: usize,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        let js = "(() => Array.from(document.querySelectorAll('body *')).filter(e => { const r = e.getBoundingClientRect(); const s = getComputedStyle(e); return r.bottom >= 0 && r.top <= innerHeight && r.width > 0 && r.height > 0 && s.display !== 'none' && s.visibility !== 'hidden'; }).map(e => (e.innerText || '').trim().replace(/\\s+/g, ' ')).filter(Boolean).slice(0, 80).join('\\n'))()";
        let result = self.page.evaluate(js).await?;
        let mut out = self
            .state_header("read_viewport -> complete", "complete", max_bytes)
            .await?;
        out.push_str("\nuntrusted_viewport_text:\n");
        if let Some(value) = result.value().and_then(serde_json::Value::as_str) {
            out.push_str(&cap_chars(value, max_chars));
            out.push('\n');
        } else {
            out.push_str("none\n");
        }
        Ok(cap(out, max_bytes))
    }

    async fn wait_for(
        &mut self,
        text: Option<&str>,
        state: Option<&str>,
        timeout_ms: Option<u64>,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        let limit = Duration::from_millis(timeout_ms.unwrap_or(5_000).clamp(100, 30_000));
        let started = Instant::now();
        loop {
            if let Some(expected) = text {
                let literal = serde_json::to_string(expected)
                    .map_err(|err| BrowserError::Message(err.to_string()))?;
                let js = format!("document.body && document.body.innerText.includes({literal})");
                if self
                    .page
                    .evaluate(js.as_str())
                    .await?
                    .value()
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
                {
                    return self
                        .snapshot("wait_for -> changed", "complete", false, max_bytes)
                        .await;
                }
            } else if let Some(expected) = state {
                match expected {
                    "usable" | "idle" => {
                        return self
                            .snapshot("wait_for -> complete", "complete", false, max_bytes)
                            .await;
                    }
                    "complete" => {
                        if self.ready_state_complete().await? {
                            return self
                                .snapshot("wait_for -> complete", "complete", false, max_bytes)
                                .await;
                        }
                    }
                    other => {
                        return Err(BrowserError::Input(format!(
                            "unsupported wait_for state '{other}'; valid states: usable, idle, complete"
                        )));
                    }
                }
            } else {
                return Err(BrowserError::Input(
                    "action 'wait_for' requires 'text' or 'state'".to_owned(),
                ));
            }
            if started.elapsed() >= limit {
                return self
                    .snapshot("wait_for -> timeout", "timeout", false, max_bytes)
                    .await;
            }
            sleep(Duration::from_millis(200)).await;
        }
    }

    async fn debug_eval(&self, js: &str, max_bytes: usize) -> Result<String, BrowserError> {
        let result = self.page.evaluate(js).await?;
        let rendered = match result.value() {
            Some(value) => {
                serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
            }
            None => "undefined".to_owned(),
        };
        Ok(cap(rendered, max_bytes))
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

    async fn settle_and_snapshot(
        &mut self,
        last_action: &str,
        load: &str,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        let _ = timeout(SETTLE_TIMEOUT, self.page.wait_for_navigation()).await;
        let switched = self.follow_new_tab().await;
        let _ = self.page.bring_to_front().await;
        self.snapshot(last_action, load, switched, max_bytes).await
    }

    async fn snapshot(
        &mut self,
        last_action: &str,
        load: &str,
        switched: bool,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        let raw = timeout(SNAPSHOT_TIMEOUT, run_snapshot(&self.page))
            .await
            .map_err(|_| BrowserError::Message("snapshot timed out".to_owned()))??;
        let url = self.page.url().await.ok().flatten().unwrap_or_default();
        self.snapshot_seq = self.snapshot_seq.saturating_add(1);
        let snapshot_id = format!("s{}", self.snapshot_seq);
        self.current_snapshot_id = Some(snapshot_id.clone());
        Ok(format_snapshot(
            &BrowserSnapshot {
                snapshot_id: &snapshot_id,
                url: &url,
                state: "usable",
                load,
                profile: "persistent",
                last_action: Some(last_action),
                switched,
                raw: &raw,
            },
            max_bytes.min(SNAPSHOT_MAX_BYTES),
        ))
    }

    async fn state_header(
        &self,
        last_action: &str,
        load: &str,
        max_bytes: usize,
    ) -> Result<String, BrowserError> {
        let url = self.page.url().await.ok().flatten().unwrap_or_default();
        let title = self
            .page
            .evaluate("document.title || ''")
            .await
            .ok()
            .and_then(|result| {
                result
                    .value()
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_default();
        let snapshot_id = self.current_snapshot_id.as_deref().unwrap_or("none");
        let mut out = String::new();
        let _ = writeln!(out, "snapshot_id: {snapshot_id}");
        let _ = writeln!(out, "url: {url}");
        let _ = writeln!(out, "title: {title}");
        out.push_str("state: usable\n");
        let _ = writeln!(out, "load: {load}");
        out.push_str("profile: persistent\n");
        let _ = writeln!(out, "\nlast_action: {last_action}");
        out.push_str("\nwarnings:\n- page_content_untrusted\n- refs_expire_after_next_snapshot\n");
        Ok(cap(out, max_bytes))
    }

    fn validate_snapshot(&self, reference: &BrowserRef) -> Result<(), BrowserError> {
        if let Some(expected) = &reference.snapshot_id
            && self.current_snapshot_id.as_ref() != Some(expected)
        {
            return Err(BrowserError::Input(format!(
                "stale ref {}:{}; current snapshot is {}",
                expected,
                reference.reference,
                self.current_snapshot_id.as_deref().unwrap_or("none")
            )));
        }
        Ok(())
    }

    async fn ready_state_complete(&self) -> Result<bool, BrowserError> {
        Ok(self
            .page
            .evaluate("document.readyState === 'complete'")
            .await?
            .value()
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false))
    }

    async fn follow_new_tab(&mut self) -> bool {
        let Ok(pages) = self.browser.pages().await else {
            return false;
        };
        let count = pages.len();
        let mut switched = false;
        if count > self.page_count
            && let Some(last) = pages.into_iter().last()
        {
            self.page = last;
            switched = true;
        }
        self.page_count = count;
        switched
    }
}

async fn run_snapshot(page: &Page) -> Result<RawSnapshot, BrowserError> {
    let params = EvaluateParams::builder()
        .expression(SNAPSHOT_JS)
        .return_by_value(true)
        .build()
        .map_err(|err| BrowserError::Message(format!("snapshot eval build: {err}")))?;
    let result = page.evaluate(params).await?;
    result
        .into_value()
        .map_err(|err| BrowserError::Message(format!("could not parse snapshot: {err}")))
}

async fn ensure_actionable(element: &Element, action: &str) -> Result<(), BrowserError> {
    let returns = element
        .call_js_fn(
            "function() { const r = this.getBoundingClientRect(); const s = getComputedStyle(this); const tag = this.tagName.toLowerCase(); const type = (this.getAttribute('type') || '').toLowerCase(); return { visible: r.width > 0 && r.height > 0 && s.display !== 'none' && s.visibility !== 'hidden' && s.opacity !== '0', disabled: !!this.disabled || this.getAttribute('aria-disabled') === 'true', readonly: !!this.readOnly, editable: tag === 'textarea' || tag === 'select' || this.isContentEditable || (tag === 'input' && type !== 'button' && type !== 'submit' && type !== 'reset'), selectable: tag === 'select' }; }",
            false,
        )
        .await?;
    let Some(value) = returns.result.value.as_ref() else {
        return Err(BrowserError::Input(format!(
            "cannot determine whether ref is actionable for {action}"
        )));
    };
    let visible = value
        .get("visible")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let disabled = value
        .get("disabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let readonly = value
        .get("readonly")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let editable = value
        .get("editable")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let selectable = value
        .get("selectable")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !visible {
        return Err(BrowserError::Input(format!(
            "ref is not visible for {action}; take a new snapshot or scroll"
        )));
    }
    if disabled {
        return Err(BrowserError::Input(format!(
            "ref is disabled for {action}; choose another element"
        )));
    }
    if action == "fill" && (!editable || readonly) {
        return Err(BrowserError::Input(
            "ref is not editable for fill; choose a textbox-like element".to_owned(),
        ));
    }
    if action == "select" && !selectable {
        return Err(BrowserError::Input(
            "ref is not a select element for select; choose a combobox".to_owned(),
        ));
    }
    Ok(())
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

fn cap(mut text: String, max_bytes: usize) -> String {
    if text.len() > max_bytes {
        let boundary = text.floor_char_boundary(max_bytes);
        text.truncate(boundary);
        text.push_str("\n[output truncated]");
    }
    text
}

fn cap_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
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
    use super::normalize_url;

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
