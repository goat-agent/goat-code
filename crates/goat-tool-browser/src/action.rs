use serde::Deserialize;

use crate::error::BrowserError;
use crate::snapshot::{RefParts, parse_ref};

#[derive(Debug, Deserialize)]
struct Input {
    action: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default, rename = "ref")]
    reference: Option<String>,
    #[serde(default)]
    snapshot_id: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    submit: bool,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    js: Option<String>,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    amount: Option<i64>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    max_chars: Option<usize>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserRef {
    pub snapshot_id: Option<String>,
    pub reference: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Navigate {
        url: String,
    },
    Snapshot,
    Click {
        reference: BrowserRef,
    },
    Fill {
        reference: BrowserRef,
        text: String,
        submit: bool,
    },
    Select {
        reference: BrowserRef,
        value: String,
    },
    PressKey {
        key: String,
    },
    Scroll {
        direction: ScrollDirection,
        amount: Option<i64>,
    },
    GoBack,
    GoForward,
    FindText {
        query: String,
        max_chars: Option<usize>,
    },
    Inspect {
        reference: BrowserRef,
        max_chars: Option<usize>,
    },
    ReadViewport {
        max_chars: Option<usize>,
    },
    WaitFor {
        text: Option<String>,
        state: Option<String>,
        timeout_ms: Option<u64>,
    },
    Screenshot,
    DebugEval {
        js: String,
    },
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}

pub fn parse(input: &str) -> Result<Action, BrowserError> {
    let raw: Input = serde_json::from_str(input)
        .map_err(|err| BrowserError::Input(format!("invalid tool input: {err}")))?;
    let action = match raw.action.as_str() {
        "navigate" => Action::Navigate {
            url: require(raw.url, "navigate", "url")?,
        },
        "snapshot" => Action::Snapshot,
        "click" => Action::Click {
            reference: require_ref(raw.reference, raw.snapshot_id, "click")?,
        },
        "fill" => Action::Fill {
            reference: require_ref(raw.reference, raw.snapshot_id, "fill")?,
            text: require(raw.text, "fill", "text")?,
            submit: raw.submit,
        },
        "select" => Action::Select {
            reference: require_ref(raw.reference, raw.snapshot_id, "select")?,
            value: require(raw.value, "select", "value")?,
        },
        "press_key" => Action::PressKey {
            key: require(raw.key, "press_key", "key")?,
        },
        "scroll" => Action::Scroll {
            direction: require_direction(raw.direction)?,
            amount: raw.amount,
        },
        "go_back" => Action::GoBack,
        "go_forward" => Action::GoForward,
        "find_text" => Action::FindText {
            query: require(raw.query, "find_text", "query")?,
            max_chars: raw.max_chars,
        },
        "inspect" => Action::Inspect {
            reference: require_ref(raw.reference, raw.snapshot_id, "inspect")?,
            max_chars: raw.max_chars,
        },
        "read_viewport" => Action::ReadViewport {
            max_chars: raw.max_chars,
        },
        "wait_for" => Action::WaitFor {
            text: raw.text,
            state: raw.state,
            timeout_ms: raw.timeout_ms,
        },
        "screenshot" => Action::Screenshot,
        "close" => Action::Close,
        "debug_eval" => Action::DebugEval {
            js: require(raw.js, "debug_eval", "js")?,
        },
        other => {
            return Err(BrowserError::Input(format!(
                "unknown action '{other}'; valid actions: navigate, snapshot, click, fill, select, press_key, scroll, go_back, go_forward, find_text, inspect, read_viewport, wait_for, screenshot, close, debug_eval"
            )));
        }
    };
    Ok(action)
}

fn require(value: Option<String>, action: &str, field: &str) -> Result<String, BrowserError> {
    value
        .filter(|s| !s.is_empty())
        .ok_or_else(|| BrowserError::Input(format!("action '{action}' requires '{field}'")))
}

fn require_ref(
    value: Option<String>,
    snapshot_id: Option<String>,
    action: &str,
) -> Result<BrowserRef, BrowserError> {
    let raw = require(value, action, "ref")?;
    let RefParts {
        snapshot_id: parsed_snapshot,
        reference,
    } = parse_ref(&raw).ok_or_else(|| {
        BrowserError::Input(format!(
            "action '{action}' got an invalid ref '{raw}'; refs look like s12:e3 and come from the latest snapshot"
        ))
    })?;
    let snapshot_id = parsed_snapshot.or(snapshot_id);
    Ok(BrowserRef {
        snapshot_id,
        reference,
    })
}

fn require_direction(value: Option<String>) -> Result<ScrollDirection, BrowserError> {
    let direction = require(value, "scroll", "direction")?;
    match direction.as_str() {
        "up" => Ok(ScrollDirection::Up),
        "down" => Ok(ScrollDirection::Down),
        "left" => Ok(ScrollDirection::Left),
        "right" => Ok(ScrollDirection::Right),
        other => Err(BrowserError::Input(format!(
            "unknown scroll direction '{other}'; valid directions: up, down, left, right"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, BrowserRef, ScrollDirection, parse};

    #[test]
    fn parses_navigate() {
        let action = parse(r#"{"action":"navigate","url":"example.com"}"#).unwrap();
        assert_eq!(
            action,
            Action::Navigate {
                url: "example.com".to_owned()
            }
        );
    }

    #[test]
    fn parses_fill_with_submit() {
        let action = parse(r#"{"action":"fill","ref":"s2:e3","text":"hi","submit":true}"#).unwrap();
        assert_eq!(
            action,
            Action::Fill {
                reference: BrowserRef {
                    snapshot_id: Some("s2".to_owned()),
                    reference: "e3".to_owned(),
                },
                text: "hi".to_owned(),
                submit: true,
            }
        );
    }

    #[test]
    fn parses_scroll() {
        let action = parse(r#"{"action":"scroll","direction":"down","amount":640}"#).unwrap();
        assert_eq!(
            action,
            Action::Scroll {
                direction: ScrollDirection::Down,
                amount: Some(640),
            }
        );
    }

    #[test]
    fn rejects_legacy_type() {
        let err = parse(r#"{"action":"type","ref":"e3","text":"hi"}"#).unwrap_err();
        assert!(err.to_string().contains("unknown action"));
    }

    #[test]
    fn click_requires_ref() {
        let err = parse(r#"{"action":"click"}"#).unwrap_err();
        assert!(err.to_string().contains("requires 'ref'"));
    }

    #[test]
    fn click_rejects_bad_ref() {
        let err = parse(r#"{"action":"click","ref":"e1' or '1"}"#).unwrap_err();
        assert!(err.to_string().contains("invalid ref"));
    }

    #[test]
    fn rejects_unknown_action() {
        let err = parse(r#"{"action":"teleport"}"#).unwrap_err();
        assert!(err.to_string().contains("unknown action"));
    }

    #[test]
    fn rejects_malformed_json() {
        let err = parse("not json").unwrap_err();
        assert!(err.to_string().contains("invalid tool input"));
    }
}
