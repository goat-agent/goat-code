use goat_protocol::ToolOutcome;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    highlight::Highlighter, layout::format_tokens, markdown, overlay::truncate_to_width, symbols,
    theme::Theme, wrap,
};

use super::ImagePlacement;
use super::item::{Item, ShellStatus, ToolStatus, Working};

pub(super) fn is_blank(line: &Line<'_>) -> bool {
    line.spans.iter().all(|s| s.content.is_empty())
}

pub(super) fn stable_prefix_len(buffer: &str) -> usize {
    let mut in_fence = false;
    let mut offset = 0usize;
    let mut split = 0usize;
    let mut prev_blank = true;
    for line in buffer.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        let stripped = trimmed.trim_start();
        if stripped.starts_with("```") || stripped.starts_with("~~~") {
            in_fence = !in_fence;
        }
        let is_blank = trimmed.trim().is_empty();
        if is_blank && !in_fence && !prev_blank {
            split = offset + line.len();
        }
        prev_blank = is_blank;
        offset += line.len();
    }
    split.min(buffer.len())
}

fn leading_indent(line: &Line<'static>) -> Vec<Span<'static>> {
    let mut prefix: Vec<Span<'static>> = Vec::new();
    let gutter_ch = symbols::ui::QUOTE_GUTTER.chars().next().unwrap_or('▎');
    for span in &line.spans {
        let content = span.content.as_ref();
        let is_blank = !content.is_empty() && content.chars().all(|c| c == ' ');
        let is_gutter = content.starts_with(gutter_ch);
        if is_blank {
            prefix.push(Span::styled(content.to_owned(), span.style));
        } else if is_gutter {
            prefix.push(span.clone());
            break;
        } else {
            break;
        }
    }
    prefix
}

pub(super) fn hang(
    content: &[Line<'static>],
    marker: Span<'static>,
    width: u16,
) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(2);
    let mut first = Some(marker);
    if content.is_empty() {
        return vec![Line::from(vec![first.take().unwrap_or_default()])];
    }
    let mut out: Vec<Line<'static>> = Vec::new();
    for line in content {
        if line.spans.len() == 1 && line.spans[0].content.as_ref() == symbols::ui::HRULE {
            let style = line.spans[0].style;
            let prefix = first.take().unwrap_or_else(|| Span::raw("  "));
            let prefix_w = UnicodeWidthStr::width(prefix.content.as_ref());
            let rule_w = usize::from(width).saturating_sub(prefix_w).max(1);
            out.push(Line::from(vec![
                prefix,
                Span::styled("─".repeat(rule_w), style),
            ]));
            continue;
        }
        let indent = leading_indent(line);
        let mut wrapped = wrap::wrap_line(line, inner).into_iter();
        if let Some(mut first_row) = wrapped.next() {
            let prefix = first.take().unwrap_or_else(|| Span::raw("  "));
            first_row.spans.insert(0, prefix);
            out.push(first_row);
        }
        for mut row in wrapped {
            for (i, span) in indent.iter().enumerate() {
                row.spans.insert(i, span.clone());
            }
            row.spans.insert(0, Span::raw("  "));
            out.push(row);
        }
    }
    out
}

pub(super) fn plain_lines(text: &str, theme: Theme) -> Vec<Line<'static>> {
    text.split('\n')
        .map(|raw| Line::from(Span::styled(raw.to_owned(), theme.base())))
        .collect()
}

fn plain_lines_styled(text: &str, style: ratatui::style::Style) -> Vec<Line<'static>> {
    text.split('\n')
        .map(|raw| Line::from(Span::styled(raw.to_owned(), style)))
        .collect()
}

pub(super) fn format_elapsed(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

const QUEUED_ROW_CAP: usize = 3;

pub(super) fn queued_rows(theme: Theme, width: u16, queued: &[String]) -> Vec<Line<'static>> {
    let inner = usize::from(width.saturating_sub(2));
    let mut rows: Vec<Line<'static>> = Vec::new();
    for label in queued.iter().take(QUEUED_ROW_CAP) {
        rows.push(Line::from(vec![
            Span::styled(symbols::marker::USER, theme.muted()),
            Span::styled(truncate_to_width(label, inner), theme.muted()),
        ]));
    }
    if queued.len() > QUEUED_ROW_CAP {
        rows.push(Line::from(Span::styled(
            format!(
                "{} {} more queued",
                symbols::ui::ELLIPSIS,
                queued.len() - QUEUED_ROW_CAP
            ),
            theme.muted(),
        )));
    }
    rows
}

pub(super) fn working_rows(
    theme: Theme,
    width: u16,
    spinner: &'static str,
    w: &Working,
) -> Vec<Line<'static>> {
    let label = w.label.clone().unwrap_or_else(|| {
        let verb = if w.thinking { "thinking" } else { "working" };
        format!("{verb}{}", symbols::ui::ELLIPSIS)
    });
    let mut spans = vec![Span::styled(label, theme.muted())];
    if let Some(secs) = w.elapsed {
        spans.push(Span::styled(
            format!("{}{}", symbols::ui::SEPARATOR, format_elapsed(secs)),
            theme.muted(),
        ));
    }
    if let Some(tokens) = w.tokens {
        spans.push(Span::styled(
            format!("{}{} tok", symbols::ui::SEPARATOR, format_tokens(tokens)),
            theme.muted(),
        ));
    }
    hang(
        &[Line::from(spans)],
        Span::styled(format!("{spinner} "), theme.accent()),
        width,
    )
}

pub(super) struct ItemMemo {
    pub(super) sig: u64,
    pub(super) rows: Vec<Line<'static>>,
}

pub(super) fn build_static_lines(
    items: &[Item],
    theme: Theme,
    width: u16,
    hl: &dyn Highlighter,
    memo: &mut Vec<ItemMemo>,
) -> (Vec<Line<'static>>, Vec<usize>, Vec<ImagePlacement>) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spinner_lines: Vec<usize> = Vec::new();
    let mut images: Vec<ImagePlacement> = Vec::new();
    if memo.len() > items.len() {
        memo.truncate(items.len());
    }
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            let prev_is_tool = matches!(items.get(i - 1), Some(Item::Tool { .. }));
            let cur_is_tool = matches!(item, Item::Tool { .. });
            if !(prev_is_tool && cur_is_tool) {
                lines.push(Line::default());
            }
        }
        if matches!(
            item,
            Item::Tool {
                status: ToolStatus::Running,
                ..
            } | Item::Shell {
                status: ShellStatus::Running,
                ..
            }
        ) {
            spinner_lines.push(lines.len());
        }
        let sig = item_signature(item);
        let rows = match memo.get(i) {
            Some(cached) if cached.sig == sig => cached.rows.clone(),
            _ => {
                let rows = item_rows(item, theme, width, hl);
                let entry = ItemMemo {
                    sig,
                    rows: rows.clone(),
                };
                if i < memo.len() {
                    memo[i] = entry;
                } else {
                    memo.push(entry);
                }
                rows
            }
        };
        lines.extend(rows);
        if let Item::Tool {
            image: Some(img), ..
        } = item
        {
            let rows = img.rows();
            if rows > 0 {
                images.push(ImagePlacement {
                    item: i,
                    start: lines.len(),
                    rows,
                });
                for _ in 0..rows {
                    lines.push(Line::default());
                }
            }
        }
    }
    (lines, spinner_lines, images)
}

pub(super) fn item_signature(item: &Item) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match item {
        Item::User(message) => {
            0u8.hash(&mut hasher);
            message.text.hash(&mut hasher);
            for attachment in &message.attachments {
                attachment.label.hash(&mut hasher);
                attachment.media_type.hash(&mut hasher);
                attachment.data.hash(&mut hasher);
            }
        }
        Item::Agent(text) => {
            1u8.hash(&mut hasher);
            text.hash(&mut hasher);
        }
        Item::Shell {
            command, status, ..
        } => {
            2u8.hash(&mut hasher);
            command.hash(&mut hasher);
            match status {
                ShellStatus::Running => 0u8.hash(&mut hasher),
                ShellStatus::Done(output) => {
                    1u8.hash(&mut hasher);
                    output.hash(&mut hasher);
                }
            }
        }
        Item::Error(text) => {
            3u8.hash(&mut hasher);
            text.hash(&mut hasher);
        }
        Item::Notice(text) => {
            4u8.hash(&mut hasher);
            text.hash(&mut hasher);
        }
        Item::Compaction {
            tokens_before,
            tokens_after,
        } => {
            5u8.hash(&mut hasher);
            tokens_before.hash(&mut hasher);
            tokens_after.hash(&mut hasher);
        }
        Item::Tool {
            name,
            display,
            status,
            ..
        } => {
            6u8.hash(&mut hasher);
            name.hash(&mut hasher);
            display.primary.hash(&mut hasher);
            display.detail.hash(&mut hasher);
            match status {
                ToolStatus::Running => 0u8.hash(&mut hasher),
                ToolStatus::Done(outcome) => {
                    1u8.hash(&mut hasher);
                    outcome.ok.hash(&mut hasher);
                    outcome.summary.hash(&mut hasher);
                }
            }
        }
    }
    hasher.finish()
}

pub(super) fn item_rows(
    item: &Item,
    theme: Theme,
    width: u16,
    hl: &dyn Highlighter,
) -> Vec<Line<'static>> {
    match item {
        Item::User(message) => {
            let mut lines = plain_lines(&message.text, theme);
            for attachment in &message.attachments {
                lines.push(Line::from(Span::styled(
                    format!("[image: {}]", attachment.label),
                    theme.muted(),
                )));
            }
            hang(
                &lines,
                Span::styled(symbols::marker::USER, theme.role_user()),
                width,
            )
        }
        Item::Agent(text) => {
            let rendered = markdown::render(text, theme, hl);
            let end = rendered
                .iter()
                .rposition(|l| !is_blank(l))
                .map_or(0, |i| i + 1);
            hang(
                &rendered[..end],
                Span::styled(symbols::marker::AGENT, theme.role_agent()),
                width,
            )
        }
        Item::Shell {
            command, status, ..
        } => shell_rows(command, status, theme, width),
        Item::Error(text) => hang(
            &plain_lines_styled(text, theme.error_body()),
            Span::styled(symbols::marker::ERROR, theme.error()),
            width,
        ),
        Item::Notice(text) => hang(
            &plain_lines(text, theme),
            Span::styled(symbols::marker::NOTICE, theme.muted()),
            width,
        ),
        Item::Compaction {
            tokens_before,
            tokens_after,
        } => {
            let label = format!(
                " context compacted{}{} → {} ",
                symbols::ui::SEPARATOR,
                format_tokens(u64::from(*tokens_before)),
                format_tokens(u64::from(*tokens_after)),
            );
            let total = usize::from(width).saturating_sub(2);
            let dashes = total.saturating_sub(UnicodeWidthStr::width(label.as_str()));
            let left = dashes / 2;
            let right = dashes - left;
            vec![Line::from(vec![
                Span::raw("  "),
                Span::styled("─".repeat(left), theme.muted()),
                Span::styled(label, theme.muted()),
                Span::styled("─".repeat(right), theme.muted()),
            ])]
        }
        Item::Tool {
            name,
            display,
            status,
            ..
        } => {
            let (marker, marker_style): (&str, _) = match status {
                ToolStatus::Running => (symbols::SPINNER[0], theme.accent()),
                ToolStatus::Done(ToolOutcome { ok: true, .. }) => {
                    (symbols::ui::CHECK, theme.success())
                }
                ToolStatus::Done(ToolOutcome { ok: false, .. }) => {
                    (symbols::ui::CROSS, theme.error())
                }
            };

            let verb = name.to_lowercase();
            let verb_w = verb.width();
            let avail = usize::from(width)
                .saturating_sub(2)
                .saturating_sub(verb_w)
                .saturating_sub(2);

            let primary = truncate_to_width(&display.primary, avail);
            let detail_avail = avail.saturating_sub(primary.width()).saturating_sub(2);
            let detail = display
                .detail
                .as_deref()
                .filter(|_| detail_avail > 1)
                .map(|d| truncate_to_width(d, detail_avail));

            let mut spans = vec![
                Span::styled(marker, marker_style),
                Span::raw(" "),
                Span::styled(verb, theme.role_tool()),
                Span::raw("  "),
                Span::styled(primary, theme.base()),
            ];
            if let Some(d) = detail {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(d, theme.muted()));
            }

            let mut result = vec![Line::from(spans)];
            if let ToolStatus::Done(ToolOutcome {
                summary: Some(summary),
                ..
            }) = status
            {
                result.extend(result_rows(summary, theme, width));
            }
            result
        }
    }
}

const RESULT_BLOCK_CAP: usize = 6;
pub(super) const SHELL_BLOCK_CAP: usize = 20;
const SHELL_EXIT_PREFIX: &str = "exit code: ";
const SHELL_NO_OUTPUT: &str = "(no output)";

pub(super) fn resolve_carriage_returns(line: &str) -> &str {
    let line = line.strip_suffix('\r').unwrap_or(line);
    line.rsplit('\r').next().unwrap_or(line)
}

pub(super) fn strip_control_sequences(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    for next in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

pub(super) fn sanitize_shell_output(output: &str) -> Vec<String> {
    let mut lines: Vec<String> = output
        .split('\n')
        .map(|line| strip_control_sequences(resolve_carriage_returns(line)))
        .collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines
}

pub(super) fn shell_line_style(line: &str, theme: Theme) -> Style {
    if line.starts_with(SHELL_EXIT_PREFIX) || (line.starts_with('[') && line.ends_with(']')) {
        theme.error()
    } else if line.starts_with("+ ") {
        theme.role_agent()
    } else if line.starts_with("- ") {
        theme.error()
    } else {
        theme.muted()
    }
}

pub(super) fn shell_rows(
    command: &str,
    status: &ShellStatus,
    theme: Theme,
    width: u16,
) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(2);
    let (marker, marker_style) = match status {
        ShellStatus::Running => (symbols::SPINNER[0], theme.accent()),
        ShellStatus::Done(_) => (symbols::ui::BANG, theme.shell()),
    };
    let mut out = hang(
        &plain_lines(command, theme),
        Span::styled(format!("{marker} "), marker_style),
        width,
    );

    let ShellStatus::Done(output) = status else {
        return out;
    };
    let lines = sanitize_shell_output(output);
    if lines.is_empty() {
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(SHELL_NO_OUTPUT, theme.muted()),
        ]));
        return out;
    }
    let exit_line = lines
        .last()
        .filter(|line| line.starts_with(SHELL_EXIT_PREFIX))
        .cloned();
    let mut rows: Vec<Line<'static>> = Vec::new();
    for line in &lines {
        let content = Line::from(Span::styled(
            line.replace('\t', "  "),
            shell_line_style(line, theme),
        ));
        for mut row in wrap::wrap_line(&content, inner) {
            row.spans.insert(0, Span::raw("  "));
            rows.push(row);
        }
    }
    let total = rows.len();
    if total > SHELL_BLOCK_CAP {
        rows.truncate(SHELL_BLOCK_CAP);
        rows.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{} {} more", symbols::ui::ELLIPSIS, total - SHELL_BLOCK_CAP),
                theme.muted(),
            ),
        ]));
        if let Some(exit) = exit_line {
            rows.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(exit, theme.error()),
            ]));
        }
    }
    out.extend(rows);
    out
}

pub(super) fn result_rows(summary: &str, theme: Theme, width: u16) -> Vec<Line<'static>> {
    let src: Vec<&str> = summary.lines().collect();
    let inner = width.saturating_sub(2);
    let mut out: Vec<Line<'static>> = Vec::new();
    for line in src.iter().take(RESULT_BLOCK_CAP) {
        let style = if line.starts_with("+ ") {
            theme.role_agent()
        } else if line.starts_with("- ") {
            theme.error()
        } else {
            theme.muted()
        };
        let content = Line::from(Span::styled(line.replace('\t', "  "), style));
        for mut row in wrap::wrap_line(&content, inner) {
            row.spans.insert(0, Span::raw("  "));
            out.push(row);
        }
    }
    if src.len() > RESULT_BLOCK_CAP {
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!(
                    "{} {} more",
                    symbols::ui::ELLIPSIS,
                    src.len() - RESULT_BLOCK_CAP
                ),
                theme.muted(),
            ),
        ]));
    }
    out
}
