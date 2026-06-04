use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

use crate::{app::App, theme::Theme};

pub fn render(frame: &mut Frame, app: &mut App) {
    let theme = app.theme();
    let full = frame.area();
    frame.render_widget(Block::new().style(theme.base()), full);

    let area = full.inner(Margin {
        horizontal: 1,
        vertical: 0,
    });
    let [header, body, composer, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(app.composer_height()),
        Constraint::Length(2),
    ])
    .areas(area);

    app.clamp_scroll(body.height, body.width);

    render_header(frame, header, app, theme);
    app.transcript().render(frame, body, theme, app.scroll());
    app.composer().render(frame, composer, theme);
    render_footer(frame, footer, app, theme);
}

fn render_header(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {}", app.cwd()),
            theme.muted(),
        ))),
        area,
    );
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let line = if app.is_busy() {
        Line::from(vec![
            Span::styled(format!(" {} ", app.spinner_frame()), theme.accent()),
            Span::styled("Working…", theme.muted()),
            Span::styled(" · ", theme.muted()),
            Span::styled("⌃c", theme.muted()),
            Span::styled(" interrupt", theme.muted()),
        ])
    } else if app.quit_armed() {
        Line::from(Span::styled(" ⌃c again to quit", theme.muted()))
    } else {
        Line::from(vec![
            Span::styled(" ⇧↵", theme.muted()),
            Span::styled(" newline", theme.muted()),
            Span::styled(" · ", theme.muted()),
            Span::styled("↑↓", theme.muted()),
            Span::styled(" history", theme.muted()),
            Span::styled(" · ", theme.muted()),
            Span::styled("⌃c", theme.muted()),
            Span::styled(" quit", theme.muted()),
        ])
    };
    let row = Rect { height: 1, ..area };
    frame.render_widget(Paragraph::new(line), row);
}
