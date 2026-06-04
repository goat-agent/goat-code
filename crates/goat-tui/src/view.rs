use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

use crate::{app::App, theme::Theme};

pub fn render(frame: &mut Frame, app: &App) {
    let theme = app.theme();
    let area = frame.area();
    frame.render_widget(Block::new().style(theme.base()), area);

    let [header, body, composer, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(app.composer_height()),
        Constraint::Length(2),
    ])
    .areas(area);

    render_header(frame, header, app, theme);
    app.transcript().render(frame, body, theme);
    app.composer().render(frame, composer, theme);
    render_footer(frame, footer, app, theme);
}

fn render_header(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{} ", app.cwd()),
            theme.muted(),
        )))
        .right_aligned(),
        area,
    );
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let line = if app.is_busy() {
        Line::from(vec![
            Span::styled(format!(" {} ", app.spinner_frame()), theme.accent()),
            Span::styled("Working…", theme.muted()),
        ])
    } else if app.quit_armed() {
        Line::from(Span::styled(" ⌃C again to quit", theme.muted()))
    } else {
        return;
    };

    let row = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(line), row);
}
