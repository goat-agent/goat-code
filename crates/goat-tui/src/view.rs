use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

use crate::{app::App, login::Login, picker::Picker, theme::Theme};

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
        Constraint::Length(app.composer_height(area.width)),
        Constraint::Length(1),
    ])
    .areas(area);

    render_header(frame, header, app, theme);

    let overlay_height = app
        .picker()
        .map(Picker::desired_height)
        .or_else(|| app.login().map(Login::desired_height));

    if let Some(height) = overlay_height {
        let height = height.min(body.height.saturating_sub(1)).max(1);
        let [transcript_area, panel] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(height)]).areas(body);
        app.clamp_scroll(transcript_area.height, transcript_area.width);
        app.transcript()
            .render(frame, transcript_area, theme, app.scroll());
        if let Some(picker) = app.picker() {
            picker.render(frame, panel, theme);
        }
        if let Some(login) = app.login() {
            login.render(frame, panel, theme);
        }
        app.composer().render(frame, composer, theme, false);
    } else {
        app.clamp_scroll(body.height, body.width);
        app.transcript().render(frame, body, theme, app.scroll());
        app.composer().render(frame, composer, theme, true);
    }

    render_footer(frame, footer, app, theme);
}

fn render_header(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let mut spans = vec![Span::styled(format!(" {}", app.cwd()), theme.muted())];
    if let Some(model) = app.current_model() {
        spans.push(Span::styled("  ·  ", theme.muted()));
        spans.push(Span::styled(
            format!("{}/{}", model.provider, model.model),
            theme.muted(),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let line = if app.is_busy() {
        let mut spans = vec![
            Span::styled(format!(" {} ", app.spinner_frame()), theme.accent()),
            Span::styled("Working…", theme.muted()),
        ];
        if let Some(secs) = app.elapsed_secs() {
            spans.push(Span::styled(format!(" {secs}s"), theme.muted()));
        }
        spans.push(Span::styled(" · ", theme.muted()));
        spans.push(Span::styled("⌃c", theme.key()));
        spans.push(Span::styled(" interrupt", theme.muted()));
        Line::from(spans)
    } else if app.quit_armed() {
        Line::from(vec![
            Span::styled(" ⌃c", theme.key()),
            Span::styled(" again to quit", theme.muted()),
        ])
    } else {
        Line::from(vec![
            Span::styled(" ⇧↵", theme.key()),
            Span::styled(" newline", theme.muted()),
            Span::styled(" · ", theme.muted()),
            Span::styled("↑↓", theme.key()),
            Span::styled(" history", theme.muted()),
            Span::styled(" · ", theme.muted()),
            Span::styled("⌃c", theme.key()),
            Span::styled(" quit", theme.muted()),
        ])
    };
    frame.render_widget(Paragraph::new(line), area);
}
