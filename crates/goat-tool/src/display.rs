use goat_protocol::ToolDisplay;
use serde_json::Value;

pub fn raw(input: &str) -> ToolDisplay {
    ToolDisplay::primary(flatten(input))
}

pub fn flatten(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

const PRIORITY_KEYS: [&str; 8] = [
    "path",
    "file_path",
    "command",
    "pattern",
    "query",
    "url",
    "action",
    "name",
];

pub fn generic(input: &str) -> ToolDisplay {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(input) else {
        return raw(input);
    };
    let mut parts: Vec<String> = Vec::new();
    let mut used: Vec<&str> = Vec::new();
    for key in PRIORITY_KEYS {
        if let Some(text) = map.get(key).and_then(scalar_text) {
            parts.push(text);
            used.push(key);
        }
    }
    for (key, value) in &map {
        if parts.len() >= 3 {
            break;
        }
        if used.iter().any(|k| k == key) {
            continue;
        }
        if let Some(text) = scalar_text(value) {
            parts.push(text);
        }
    }
    let mut iter = parts.into_iter();
    let Some(primary) = iter.next() else {
        return raw(input);
    };
    let rest: Vec<String> = iter.collect();
    if rest.is_empty() {
        ToolDisplay::primary(primary)
    } else {
        ToolDisplay::with_detail(primary, rest.join(" · "))
    }
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.is_empty() => Some(flatten(s)),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use goat_protocol::ToolDisplay;

    use super::generic;

    #[test]
    fn generic_prefers_priority_keys() {
        let got = generic(r#"{"limit":5,"query":"rust tui"}"#);
        assert_eq!(got.primary, "rust tui");
        assert_eq!(got.detail.as_deref(), Some("5"));
    }

    #[test]
    fn generic_flattens_whitespace() {
        let got = generic(r#"{"command":"cargo \n  build"}"#);
        assert_eq!(got.primary, "cargo build");
    }

    #[test]
    fn non_json_passes_raw_through() {
        assert_eq!(generic("src/lib.rs"), ToolDisplay::primary("src/lib.rs"));
    }

    #[test]
    fn empty_object_passes_raw_through() {
        assert_eq!(generic("{}"), ToolDisplay::primary("{}"));
    }
}
