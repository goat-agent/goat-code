use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    text::{Line, Span},
    widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

use crate::{
    app::{App, Overlay},
    overlay, symbols,
    theme::Theme,
};

#[allow(clippy::too_many_lines)]
pub fn render(frame: &mut Frame, app: &mut App) {
    let theme = app.theme();
    let full = frame.area();
    frame.render_widget(Block::new().style(theme.base()), full);

    let area = full.inner(Margin {
        horizontal: 1,
        vertical: 0,
    });

    let composer_h = app.composer_height(area.width);

    if let Overlay::Commands(menu) = app.overlay() {
        let panel_h = menu
            .desired_height()
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
        if let Overlay::Commands(menu) = app.overlay() {
            menu.render(frame, panel, theme);
        }
        app.composer().render(frame, composer_area, theme, true);
        render_toasts(frame, area, app, theme);
        return;
    }

    match app.overlay() {
        Overlay::Config(_) | Overlay::Model(_) | Overlay::Effort(_) | Overlay::Thread(_) => {
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
            match app.overlay() {
                Overlay::Config(config) => config.render(frame, body, theme),
                Overlay::Model(picker) => picker.render(frame, body, theme),
                Overlay::Effort(picker) => picker.render(frame, body, theme),
                Overlay::Thread(picker) => picker.render(frame, body, theme),
                _ => {}
            }
            app.composer().render(frame, composer, theme, false);
            render_toasts(frame, area, app, theme);
            return;
        }
        _ => {}
    }

    if let Some(cursor) = app.agent_selector() {
        let count = u16::try_from(app.agent_runs().len())
            .unwrap_or(1)
            .clamp(1, 8);
        let [header, body, composer, panel] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(composer_h),
            Constraint::Length(count),
        ])
        .areas(area);
        render_header(frame, header, app, theme);
        app.clamp_scroll(body.height, body.width);
        app.transcript()
            .render(frame, body, theme, app.scroll(), app.spinner_frame());
        render_scrollbar(frame, body, app, theme);
        render_agent_panel(frame, panel, app, theme, cursor);
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

fn render_agent_panel(frame: &mut Frame, area: Rect, app: &App, theme: Theme, cursor: usize) {
    let spinner = app.spinner_frame();
    let inner_width = usize::from(area.width);
    let lines: Vec<Line> = app
        .agent_runs()
        .iter()
        .enumerate()
        .map(|(i, run)| {
            let selected = i == cursor;
            let (marker, marker_style) = match run.done {
                None => (spinner, theme.accent()),
                Some(true) => (symbols::marker::OK, theme.role_tool()),
                Some(false) => (symbols::marker::ERROR, theme.error()),
            };
            let name_style = if selected { theme.key() } else { theme.muted() };
            let mut left = vec![
                Span::styled(marker, marker_style),
                Span::raw(" "),
                Span::styled(run.agent_type.clone(), name_style),
            ];
            if !run.label.is_empty() {
                left.push(Span::styled("  ", theme.muted()));
                left.push(Span::styled(run.label.clone(), theme.muted()));
            }
            overlay::selection_row(theme, selected, inner_width, left, None)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
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
        let model_label = if app.provider_has_multiple_accounts(&model.provider) {
            format!("{}:{}/{}", model.provider, model.account, model.model)
        } else {
            format!("{}/{}", model.provider, model.model)
        };
        spans.push(Span::styled(model_label, theme.key()));
        if let Some(effort) = model.effort {
            spans.push(Span::styled(
                format!("{}{}", symbols::ui::SEPARATOR, effort),
                theme.accent(),
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let sep = symbols::ui::SEPARATOR;
    let line = if app.is_busy() {
        let mut spans = vec![Span::styled(
            format!(" {} ", app.spinner_frame()),
            theme.accent(),
        )];
        if let Some(status) = app.agent_status() {
            spans.push(Span::styled(status, theme.muted()));
        } else {
            spans.push(Span::styled(
                format!("Working{}", symbols::ui::ELLIPSIS),
                theme.muted(),
            ));
            if let Some(secs) = app.elapsed_secs() {
                spans.push(Span::styled(format!(" {secs}s"), theme.muted()));
            }
        }
        spans.push(Span::styled(sep, theme.muted()));
        spans.push(Span::styled(
            format!("{}c", symbols::key::CTRL),
            theme.key(),
        ));
        spans.push(Span::styled(" interrupt", theme.muted()));
        Line::from(spans)
    } else if app.quit_armed() {
        Line::from(vec![
            Span::styled(format!(" {}c", symbols::key::CTRL), theme.key()),
            Span::styled(" again to quit", theme.muted()),
        ])
    } else {
        let mut spans = vec![
            Span::styled(format!(" {}", symbols::key::SHIFT_ENTER), theme.key()),
            Span::styled(" newline", theme.muted()),
            Span::styled(sep, theme.muted()),
            Span::styled(symbols::key::ARROWS_UPDOWN, theme.key()),
            Span::styled(" history", theme.muted()),
        ];
        if !app.agent_runs().is_empty() {
            spans.push(Span::styled(sep, theme.muted()));
            spans.push(Span::styled(symbols::key::ARROW_DOWN, theme.key()));
            spans.push(Span::styled(" agents", theme.muted()));
        }
        spans.push(Span::styled(sep, theme.muted()));
        spans.push(Span::styled(
            format!("{}c", symbols::key::CTRL),
            theme.key(),
        ));
        spans.push(Span::styled(" quit", theme.muted()));
        Line::from(spans)
    };
    frame.render_widget(Paragraph::new(line), area);
}
