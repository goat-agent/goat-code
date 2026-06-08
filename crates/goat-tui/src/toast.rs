use goat_protocol::NotifyKind;
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph},
};

use crate::{symbols, theme::Theme};

pub struct Toast {
    message: String,
    kind: NotifyKind,
    ticks_left: u16,
}

const TOAST_TICKS: u16 = 33;
const TOAST_HEIGHT: u16 = 3;
const TOAST_GAP: u16 = 1;
const MIN_WIDTH: u16 = 12;

impl Toast {
    pub fn new(kind: NotifyKind, message: String) -> Self {
        Self {
            kind,
            message,
            ticks_left: TOAST_TICKS,
        }
    }

    fn icon(&self) -> &'static str {
        match self.kind {
            NotifyKind::Success => symbols::ui::CHECK,
            NotifyKind::Error => symbols::ui::CROSS,
            NotifyKind::Info => symbols::ui::MIDDOT,
        }
    }

    fn icon_style(&self, theme: Theme) -> ratatui::style::Style {
        match self.kind {
            NotifyKind::Success => theme.role_agent(),
            NotifyKind::Error => theme.error(),
            NotifyKind::Info => theme.muted(),
        }
    }
}

pub fn tick(toasts: &mut Vec<Toast>) -> bool {
    let before = toasts.len();
    for t in toasts.iter_mut() {
        t.ticks_left = t.ticks_left.saturating_sub(1);
    }
    toasts.retain(|t| t.ticks_left > 0);
    toasts.len() != before
}

pub fn render(frame: &mut Frame, area: Rect, theme: Theme, toasts: &[Toast]) {
    if toasts.is_empty() || area.width < MIN_WIDTH + 4 || area.height < TOAST_HEIGHT + 1 {
        return;
    }
    let max_width = area.width.saturating_sub(4);
    let mut y = area.y.saturating_add(1);
    for toast in toasts.iter().rev() {
        if y.saturating_add(TOAST_HEIGHT) > area.bottom() {
            break;
        }
        let message_width = u16::try_from(toast.message.chars().count()).unwrap_or(u16::MAX);
        let width = message_width.saturating_add(8).clamp(MIN_WIDTH, max_width);
        let x = area.right().saturating_sub(width).saturating_sub(2);
        let rect = Rect {
            x,
            y,
            width,
            height: TOAST_HEIGHT,
        };
        frame.render_widget(Clear, rect);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme.border())
            .style(theme.base());
        let inner = block.inner(rect);
        frame.render_widget(block, rect);
        let line = Line::from(vec![
            Span::styled(" ", theme.base()),
            Span::styled(toast.icon(), toast.icon_style(theme)),
            Span::styled("  ", theme.base()),
            Span::styled(toast.message.clone(), theme.base()),
        ]);
        frame.render_widget(Paragraph::new(line), inner);
        y = y.saturating_add(TOAST_HEIGHT + TOAST_GAP);
    }
}
