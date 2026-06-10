use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Clear},
};
use unicode_width::UnicodeWidthStr;

use crate::{symbols, theme::Theme};

pub fn clamp_u16(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

pub fn overlay_frame(frame: &mut Frame, area: Rect, theme: Theme) -> Option<Rect> {
    frame.render_widget(Clear, area);
    frame.render_widget(Block::new().style(theme.panel()), area);
    let inner = area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    Some(inner)
}

pub fn overlay_layout(inner: Rect) -> (Rect, Rect, Rect) {
    let [context, _, body, _, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    (context, body, hint)
}

pub fn overlay_layout_plain(inner: Rect) -> (Rect, Rect) {
    let [body, _, hint] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    (body, hint)
}

pub fn hint_line<'a>(pairs: &[(&'a str, &'a str)], theme: Theme) -> Line<'a> {
    let mut spans: Vec<Span<'a>> = vec![Span::raw(" ")];
    for (i, (glyph, label)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(symbols::ui::SEPARATOR, theme.muted()));
        }
        spans.push(Span::styled(*glyph, theme.hint_key()));
        spans.push(Span::styled(format!(" {label}"), theme.muted()));
    }
    Line::from(spans)
}

pub fn selection_row<'a>(
    theme: Theme,
    selected: bool,
    inner_width: usize,
    left_spans: Vec<Span<'a>>,
    right_span: Option<Span<'a>>,
) -> Line<'a> {
    let caret = if selected {
        Span::styled(format!(" {} ", symbols::ui::CARET), theme.accent())
    } else {
        Span::raw("   ")
    };
    let left_w: usize = left_spans.iter().map(|s| s.content.width()).sum();
    let right_w = right_span.as_ref().map_or(0, |s| s.content.width());
    let caret_w = 3usize;
    let pad =
        inner_width.saturating_sub(caret_w + left_w + right_w + usize::from(right_span.is_some()));
    let row_style = if selected {
        theme.selected_row()
    } else {
        Style::default()
    };
    let mut spans = vec![caret];
    spans.extend(left_spans.into_iter().map(|s| {
        let patched = s.style.patch(row_style);
        Span::styled(s.content, patched)
    }));
    if let Some(right) = right_span {
        spans.push(Span::styled(" ".repeat(pad), row_style));
        let patched = right.style.patch(row_style);
        spans.push(Span::styled(right.content, patched));
    } else {
        spans.push(Span::styled(" ".repeat(pad), row_style));
    }
    Line::from(spans)
}

pub fn overflow_hint(start: usize, shown: usize, total: usize) -> (Option<String>, Option<String>) {
    let above = if start > 0 {
        Some(format!("{} {} more", symbols::ui::MORE_ABOVE, start))
    } else {
        None
    };
    let remaining = total.saturating_sub(start + shown);
    let below = if remaining > 0 {
        Some(format!("{} {} more", symbols::ui::MORE_BELOW, remaining))
    } else {
        None
    };
    (above, below)
}

pub fn centered_rect(area: Rect, max_width: u16, max_height: u16) -> Rect {
    let width = max_width.min(area.width.saturating_sub(4));
    let height = max_height.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}
