use serde::Deserialize;

use crate::error::BrowserError;
use crate::snapshot::is_valid_ref;

#[derive(Debug, Deserialize)]
struct Input {
    action: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default, rename = "ref")]
    reference: Option<String>,
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
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Navigate {
        url: String,
    },
    Snapshot,
    Click {
        reference: String,
    },
    Type {
        reference: String,
        text: String,
        submit: bool,
    },
    Select {
        reference: String,
        value: String,
    },
    PressKey {
        key: String,
    },
    Evaluate {
        js: String,
    },
    Screenshot,
    Close,
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
            reference: require_ref(raw.reference, "click")?,
        },
        "type" => Action::Type {
            reference: require_ref(raw.reference, "type")?,
            text: require(raw.text, "type", "text")?,
            submit: raw.submit,
        },
        "select" => Action::Select {
            reference: require_ref(raw.reference, "select")?,
            value: require(raw.value, "select", "value")?,
        },
        "press_key" => Action::PressKey {
            key: require(raw.key, "press_key", "key")?,
        },
        "evaluate" => Action::Evaluate {
            js: require(raw.js, "evaluate", "js")?,
        },
        "screenshot" => Action::Screenshot,
        "close" => Action::Close,
        other => {
            return Err(BrowserError::Input(format!(
                "unknown action '{other}'; valid actions: navigate, snapshot, click, type, select, press_key, evaluate, screenshot, close"
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

fn require_ref(value: Option<String>, action: &str) -> Result<String, BrowserError> {
    let reference = require(value, action, "ref")?;
    if is_valid_ref(&reference) {
        Ok(reference)
    } else {
        Err(BrowserError::Input(format!(
            "action '{action}' got an invalid ref '{reference}'; refs look like e12 and come from the latest snapshot"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, parse};

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
    fn parses_type_with_submit() {
        let action = parse(r#"{"action":"type","ref":"e3","text":"hi","submit":true}"#).unwrap();
        assert_eq!(
            action,
            Action::Type {
                reference: "e3".to_owned(),
                text: "hi".to_owned(),
                submit: true,
            }
        );
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
