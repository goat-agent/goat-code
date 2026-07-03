use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::symbols;

use super::call_sig;

pub(crate) struct ToolLineCtx<'a> {
    pub cwd: &'a str,
    pub width: u16,
    pub failed: bool,
}

pub(crate) fn transcript_sig(
    tool_name: &str,
    display_primary: &str,
    ctx: &ToolLineCtx<'_>,
) -> String {
    let full = call_sig::normalize(tool_name, display_primary);
    let (name, args) = call_sig::parse(tool_name, &full);
    let args = shorten_args(tool_name, &args, ctx);
    call_sig::format(&name, &args)
}

fn shorten_args(tool_name: &str, args: &[String], ctx: &ToolLineCtx<'_>) -> Vec<String> {
    if args.is_empty() {
        return Vec::new();
    }
    match tool_name {
        "Read" | "Write" | "Edit" => vec![shorten_path(&args[0], ctx)],
        "Glob" => vec![shorten_text(&args[0], ctx, 64)],
        "Grep" => shorten_grep(args, ctx),
        "Bash" => vec![shorten_command(&args[0], ctx)],
        "WebFetch" => vec![shorten_text(
            &url_host(&args[0]).unwrap_or_else(|| args[0].clone()),
            ctx,
            48,
        )],
        "WebSearch" => vec![shorten_text(&args[0], ctx, 48)],
        "Skill" => vec![shorten_text(&args[0], ctx, 40)],
        "Agent" => shorten_agent(args, ctx),
        "Ask" => vec![shorten_text(&args[0], ctx, 56)],
        _ => shorten_default(args, ctx),
    }
}

fn shorten_grep(args: &[String], ctx: &ToolLineCtx<'_>) -> Vec<String> {
    let mut out = vec![shorten_text(&args[0], ctx, 64)];
    if let Some(path) = args.get(1)
        && !path.is_empty()
        && path != "."
    {
        out.push(shorten_path(path, ctx));
    }
    out
}

fn shorten_agent(args: &[String], ctx: &ToolLineCtx<'_>) -> Vec<String> {
    if args.len() >= 2 {
        vec![
            shorten_text(&args[0], ctx, 24),
            shorten_text(&args[1], ctx, 40),
        ]
    } else {
        vec![shorten_text(&args[0], ctx, 48)]
    }
}

fn shorten_default(args: &[String], ctx: &ToolLineCtx<'_>) -> Vec<String> {
    let mut out = vec![clip_to_budget(&args[0], arg_budget(ctx, 64))];
    if args.len() > 1 {
        out.push("…".to_owned());
    }
    out
}

fn shorten_path(raw: &str, ctx: &ToolLineCtx<'_>) -> String {
    let rel = path_under_cwd(raw, ctx.cwd);
    ellipsize_path_middle(&rel, arg_budget(ctx, 56))
}

fn shorten_command(raw: &str, ctx: &ToolLineCtx<'_>) -> String {
    let flat: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let cap = if ctx.failed { 72 } else { 56 };
    shorten_text(&flat, ctx, cap)
}

fn shorten_text(s: &str, ctx: &ToolLineCtx<'_>, soft_max: usize) -> String {
    let clipped = clip_to_budget(s, arg_budget(ctx, soft_max));
    let budget = arg_budget(ctx, soft_max);
    if clipped.width() > budget {
        clip_to_width(&clipped, budget)
    } else {
        clipped
    }
}

fn clip_to_budget(s: &str, budget: usize) -> String {
    if s.width() <= budget {
        return s.to_owned();
    }
    clip_to_width(s, budget)
}

fn arg_budget(ctx: &ToolLineCtx<'_>, base: usize) -> usize {
    let w = usize::from(ctx.width.saturating_sub(2)).max(24);
    let cap = w.saturating_sub(8);
    let scaled = if ctx.failed {
        cap.saturating_mul(5) / 4
    } else {
        cap
    };
    base.min(scaled.max(20))
}

fn path_under_cwd(raw: &str, cwd: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return raw.to_owned();
    }
    let cwd = cwd.trim_end_matches('/');
    if cwd.is_empty() {
        return home_relative(raw);
    }
    let prefix = format!("{cwd}/");
    if let Some(rest) = raw.strip_prefix(&prefix) {
        return rest.to_owned();
    }
    if raw == cwd {
        return ".".to_owned();
    }
    home_relative(raw)
}

fn home_relative(raw: &str) -> String {
    if let Some(home) = std::env::var("HOME").ok().filter(|h| !h.is_empty()) {
        let home = home.trim_end_matches('/');
        if let Some(rest) = raw.strip_prefix(home) {
            let rest = rest.strip_prefix('/').unwrap_or(rest);
            return format!("~/{rest}");
        }
    }
    raw.to_owned()
}

fn ellipsize_path_middle(path: &str, max: usize) -> String {
    if path.width() <= max {
        return path.to_owned();
    }
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return clip_to_width(path, max);
    }
    let file = parts.last().copied().unwrap_or("");
    let parent = parts.get(parts.len().saturating_sub(2)).copied();
    if let Some(parent) = parent {
        let candidate = format!("…/{parent}/{file}");
        if candidate.width() <= max {
            return candidate;
        }
    }
    let tail = format!("…/{file}");
    if tail.width() <= max {
        return tail;
    }
    clip_to_width(file, max)
}

fn url_host(url: &str) -> Option<String> {
    let t = url.trim();
    let after = t
        .strip_prefix("https://")
        .or_else(|| t.strip_prefix("http://"))?;
    let host = after.split('/').next()?.split(':').next()?;
    (!host.is_empty()).then(|| host.to_owned())
}

fn clip_to_width(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let ell = symbols::ui::ELLIPSIS;
    let ell_w = ell.width();
    if ell_w >= max {
        return ell.to_owned();
    }
    let mut w = 0usize;
    let mut out = String::new();
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if w + cw + ell_w > max {
            break;
        }
        w += cw;
        out.push(ch);
    }
    out.push_str(ell);
    out
}

#[cfg(test)]
mod tests {
    use super::{ToolLineCtx, ellipsize_path_middle, path_under_cwd, transcript_sig};

    fn ctx(cwd: &str, width: u16) -> ToolLineCtx<'_> {
        ToolLineCtx {
            cwd,
            width,
            failed: false,
        }
    }

    #[test]
    fn read_drops_extra_args() {
        let sig = transcript_sig(
            "Read",
            "Read(/Users/jmo/proj/crates/foo/src/lib.rs, 10, 50)",
            &ctx("/Users/jmo/proj", 100),
        );
        assert!(sig.starts_with("Read("));
        assert!(sig.contains("crates/foo"));
        assert!(!sig.contains(", 10"));
    }

    #[test]
    fn grep_omits_dot_scope() {
        let sig = transcript_sig("Grep", "Grep(foo, .)", &ctx("/tmp", 80));
        assert_eq!(sig, "Grep(foo)");
    }

    #[test]
    fn glob_keeps_pattern() {
        let sig = transcript_sig("Glob", "Glob(**/symbols*)", &ctx("/x", 80));
        assert_eq!(sig, "Glob(**/symbols*)");
    }

    #[test]
    fn path_under_cwd_strips_prefix() {
        assert_eq!(
            path_under_cwd("/Users/jmo/proj/crates/a.rs", "/Users/jmo/proj"),
            "crates/a.rs"
        );
    }

    #[test]
    fn path_middle_ellipsis() {
        let s = ellipsize_path_middle("crates/goat-tui/src/transcript/tool_gist.rs", 28);
        assert!(s.contains("tool_gist.rs"));
        assert!(s.contains('…'));
    }
}
