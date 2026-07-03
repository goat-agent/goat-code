pub(crate) fn normalize(tool_name: &str, display_primary: &str) -> String {
    let trimmed = display_primary.trim();
    if trimmed.is_empty() {
        return format!("{tool_name}()");
    }
    if let Some(open) = trimmed.find('(') {
        let head = trimmed[..open].trim();
        if head == tool_name {
            return trimmed.to_owned();
        }
        if let Some(args) = bare_args(trimmed) {
            return format_with_refs(tool_name, &args);
        }
    }
    format_with_refs(tool_name, std::slice::from_ref(&trimmed))
}

pub(crate) fn parse(tool_name: &str, sig: &str) -> (String, Vec<String>) {
    let Some(open) = sig.find('(') else {
        return (tool_name.to_owned(), Vec::new());
    };
    let name = sig[..open].trim().to_owned();
    let tail = &sig[open..];
    if !tail.ends_with(')') || tail.len() < 2 {
        return (name, vec![tail.to_owned()]);
    }
    let inner = &tail[1..tail.len() - 1];
    if inner.is_empty() {
        return (name, Vec::new());
    }
    (name, inner.split(", ").map(unquote_arg).collect::<Vec<_>>())
}

pub(crate) fn format(tool_name: &str, args: &[String]) -> String {
    if args.is_empty() {
        format!("{tool_name}()")
    } else {
        let parts: Vec<String> = args
            .iter()
            .map(|a| quote_arg_if_needed(a.as_str()))
            .collect();
        format!("{tool_name}({})", parts.join(", "))
    }
}

fn bare_args(sig: &str) -> Option<Vec<&str>> {
    let open = sig.find('(')?;
    let tail = &sig[open..];
    if !tail.ends_with(')') || tail.len() < 2 {
        return None;
    }
    let inner = &tail[1..tail.len() - 1];
    if inner.is_empty() {
        return Some(Vec::new());
    }
    Some(inner.split(", ").collect())
}

fn format_with_refs(name: &str, args: &[&str]) -> String {
    if args.is_empty() {
        format!("{name}()")
    } else {
        format!("{name}({})", args.join(", "))
    }
}

fn unquote_arg(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2
        && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')))
    {
        t[1..t.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        t.to_owned()
    }
}

fn quote_arg_if_needed(s: &str) -> String {
    let needs = s.is_empty()
        || s.chars().any(char::is_whitespace)
        || s.contains('"')
        || s.contains('\'')
        || s.contains(',');
    if needs {
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        s.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{format, normalize, parse};

    #[test]
    fn normalize_wraps_bare_pattern() {
        assert_eq!(normalize("Glob", "**/symbols*"), "Glob(**/symbols*)");
    }

    #[test]
    fn normalize_keeps_existing_sig() {
        assert_eq!(
            normalize("Grep", "Grep(marker|symbols::, /tmp)"),
            "Grep(marker|symbols::, /tmp)"
        );
    }

    #[test]
    fn normalize_fixes_tool_prefix() {
        assert_eq!(normalize("Glob", "Grep(foo)"), "Glob(foo)");
    }

    #[test]
    fn parse_round_trip() {
        let sig = "Read(crates/a.rs, 1)";
        let (name, args) = parse("Read", sig);
        assert_eq!(name, "Read");
        assert_eq!(args, ["crates/a.rs", "1"]);
        assert_eq!(format(&name, &args), sig);
    }
}
