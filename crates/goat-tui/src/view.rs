use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    text::{Line, Span},
    widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

use crate::{app::App, command::CommandMenu, theme::Theme};

pub fn render(frame: &mut Frame, app: &mut App) {
    let theme = app.theme();
    let full = frame.area();
    frame.render_widget(Block::new().style(theme.base()), full);

    let area = full.inner(Margin {
        horizontal: 1,
        vertical: 0,
    });

    let composer_h = app.composer_height(area.width);

    if let Some(panel_height) = app.command_menu().map(CommandMenu::desired_height) {
        let panel_h = panel_height
            .min(area.height.saturating_sub(composer_h + 2))
            .max(1);
        let [header, transcript_area, composer_area, panel] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(composer_h),
            Constraint::Length(panel_h),
        ])
        .areas(area);

        render_header(frame, header, app, theme);
        app.clamp_scroll(transcript_area.height, transcript_area.width);
        app.transcript().render(
            frame,
            transcript_area,
            theme,
            app.scroll(),
            app.spinner_frame(),
        );
        render_scrollbar(frame, transcript_area, app, theme);
        if let Some(menu) = app.command_menu() {
            menu.render(frame, panel, theme);
        }
        app.composer().render(frame, composer_area, theme, true);
        render_toasts(frame, area, app, theme);
        return;
    }

    if app.config().is_some() || app.picker().is_some() {
        let [header, body, composer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(composer_h),
        ])
        .areas(area);
        render_header(frame, header, app, theme);
        app.clamp_scroll(body.height, body.width);
        app.transcript()
            .render(frame, body, theme, app.scroll(), app.spinner_frame());
        render_scrollbar(frame, body, app, theme);
        if let Some(config) = app.config() {
            config.render(frame, body, theme);
        }
        if let Some(picker) = app.picker() {
            picker.render(frame, body, theme);
        }
        app.composer().render(frame, composer, theme, false);
        render_toasts(frame, area, app, theme);
        return;
    }

    let [header, body, composer, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(composer_h),
        Constraint::Length(1),
    ])
    .areas(area);

    render_header(frame, header, app, theme);
    app.clamp_scroll(body.height, body.width);
    app.transcript()
        .render(frame, body, theme, app.scroll(), app.spinner_frame());
    render_scrollbar(frame, body, app, theme);
    app.composer().render(frame, composer, theme, true);
    render_footer(frame, footer, app, theme);
    render_toasts(frame, area, app, theme);
}

fn render_toasts(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    crate::toast::render(frame, area, theme, app.toasts());
}

fn render_scrollbar(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    if app.follow() {
        return;
    }
    let content_len = app.content_height(area.width);
    if content_len <= area.height {
        return;
    }
    let mut state = ScrollbarState::new(content_len as usize)
        .position(app.scroll() as usize)
        .viewport_content_length(area.height as usize);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .track_symbol(None)
            .thumb_style(theme.muted()),
        area,
        &mut state,
    );
}

fn render_header(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let mut spans = vec![Span::styled(format!(" {}", app.cwd()), theme.muted())];
    if let Some(model) = app.current_model() {
        spans.push(Span::styled("  \u{00b7}  ", theme.muted()));
        spans.push(Span::styled(
            format!("{}/{}", model.provider, model.model),
            theme.key(),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let line = if app.is_busy() {
        let mut spans = vec![
            Span::styled(format!(" {} ", app.spinner_frame()), theme.accent()),
            Span::styled("Working\u{2026}", theme.muted()),
        ];
        if let Some(secs) = app.elapsed_secs() {
            spans.push(Span::styled(format!(" {secs}s"), theme.muted()));
        }
        spans.push(Span::styled(" \u{00b7} ", theme.muted()));
        spans.push(Span::styled("\u{2303}c", theme.key()));
        spans.push(Span::styled(" interrupt", theme.muted()));
        Line::from(spans)
    } else if app.quit_armed() {
        Line::from(vec![
            Span::styled(" \u{2303}c", theme.key()),
            Span::styled(" again to quit", theme.muted()),
        ])
    } else {
        Line::from(vec![
            Span::styled(" \u{21e7}\u{21b5}", theme.key()),
            Span::styled(" newline", theme.muted()),
            Span::styled(" \u{00b7} ", theme.muted()),
            Span::styled("\u{2191}\u{2193}", theme.key()),
            Span::styled(" history", theme.muted()),
            Span::styled(" \u{00b7} ", theme.muted()),
            Span::styled("\u{2303}c", theme.key()),
            Span::styled(" quit", theme.muted()),
        ])
    };
    frame.render_widget(Paragraph::new(line), area);
}
