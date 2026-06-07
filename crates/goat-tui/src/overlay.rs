use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, BorderType, Clear},
};

use crate::theme::Theme;

pub fn clamp_u16(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

pub fn overlay_frame(
    frame: &mut Frame,
    area: Rect,
    theme: Theme,
    title: Option<&str>,
) -> Option<Rect> {
    frame.render_widget(Clear, area);
    let block = match title {
        Some(t) => Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme.border())
            .style(theme.base())
            .title_top(Line::from(Span::styled(format!(" {t} "), theme.accent())).left_aligned()),
        None => Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme.border())
            .style(theme.base()),
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    Some(inner)
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
