use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    text::{Line, Span},
    widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{App, Overlay},
    layout::{LIST_MAX, PAD_X, SCROLL_GUTTER},
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

    if let Overlay::Ask(picker, _) = app.overlay() {
        let panel_h = picker
            .desired_height()
            .min(area.height.saturating_sub(2))
            .max(3);
        let [header, transcript_area, _panel] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(panel_h),
        ])
        .areas(area);
        render_header(frame, header, app, theme);
        render_transcript(frame, transcript_area, app, theme);
        if let Overlay::Ask(picker, _) = app.overlay() {
            picker.render(frame, area, theme);
        }
        render_toasts(frame, area, app, theme);
        return;
    }

    if let Overlay::Commands(menu) = app.overlay() {
        let panel_h = menu
            .desired_height()
            .min(area.height.saturating_sub(composer_h + 2))
            .max(1);
        let [header, transcript_area, composer_area, panel] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(composer_h),
            Constraint::Length(panel_h),
        ])
        .areas(area);

        render_header(frame, header, app, theme);
        render_transcript(frame, transcript_area, app, theme);
        if let Overlay::Commands(menu) = app.overlay() {
            menu.render(frame, panel, theme);
        }
        app.composer().render(frame, composer_area, theme, true);
        render_toasts(frame, area, app, theme);
        return;
    }

    match app.overlay() {
        Overlay::Config(_)
        | Overlay::Model(_)
        | Overlay::Effort(_)
        | Overlay::Thread(_)
        | Overlay::Usage
        | Overlay::Help => {
            let [header, body, composer] = Layout::vertical([
                Constraint::Length(2),
                Constraint::Min(1),
                Constraint::Length(composer_h),
            ])
            .areas(area);
            render_header(frame, header, app, theme);
            render_transcript(frame, body, app, theme);
            match app.overlay() {
                Overlay::Config(config) => config.render(frame, body, theme),
                Overlay::Model(picker) => picker.render(frame, body, theme),
                Overlay::Effort(picker) => picker.render(frame, body, theme),
                Overlay::Thread(picker) => picker.render(frame, body, theme),
                Overlay::Usage => {
                    let view = app.build_usage_view();
                    view.render(frame, body, theme);
                }
                Overlay::Help => crate::help::render(frame, body, theme),
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
            .clamp(1, u16::try_from(LIST_MAX).unwrap_or(10));
        let [header, body, composer, panel, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(composer_h),
            Constraint::Length(count),
            Constraint::Length(1),
        ])
        .areas(area);
        render_header(frame, header, app, theme);
        render_transcript(frame, body, app, theme);
        render_agent_panel(frame, panel, app, theme, cursor);
        render_agent_footer(frame, footer, theme);
        app.composer().render(frame, composer, theme, false);
        render_toasts(frame, area, app, theme);
        return;
    }

    let [header, body, composer, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(composer_h),
        Constraint::Length(1),
    ])
    .areas(area);

    render_header(frame, header, app, theme);
    render_transcript(frame, body, app, theme);
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
                Some(true) => (symbols::ui::CHECK, theme.role_tool()),
                Some(false) => (symbols::ui::CROSS, theme.error()),
            };
            let name_style = if selected { theme.key() } else { theme.muted() };
            let mut left = vec![
                Span::styled(marker, marker_style),
                Span::raw(" "),
                Span::styled(run.agent_type.clone(), name_style),
            ];
            if !run.label.is_empty() {
                left.push(Span::styled(symbols::ui::SEPARATOR, theme.muted()));
                left.push(Span::styled(run.label.clone(), theme.muted()));
            }
            overlay::selection_row(theme, selected, inner_width, left, None)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_agent_footer(frame: &mut Frame, area: Rect, theme: Theme) {
    frame.render_widget(
        Paragraph::new(overlay::hint_line(
            &[
                (symbols::key::ARROWS_UPDOWN, "agents"),
                (symbols::key::ESC, "back"),
            ],
            theme,
        )),
        area,
    );
}

fn render_toasts(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    crate::toast::render(frame, area, theme, app.toasts());
}

fn render_transcript(frame: &mut Frame, area: Rect, app: &mut App, theme: Theme) {
    let content = Rect {
        x: area.x + PAD_X,
        y: area.y,
        width: area.width.saturating_sub(PAD_X + SCROLL_GUTTER),
        height: area.height,
    };
    app.clamp_scroll(content.height, content.width);
    let working = app.working_state();
    app.transcript().render(
        frame,
        content,
        &crate::transcript::RenderCtx {
            theme,
            scroll: app.scroll(),
            spinner: app.spinner_frame(),
            working: working.as_ref(),
            hl: &app.highlighter,
        },
    );
    if app.follow() {
        return;
    }
    let content_len = app.content_height(content.width);
    if content_len <= usize::from(content.height) {
        return;
    }
    let bar = Rect {
        x: area.x + area.width.saturating_sub(1),
        y: area.y,
        width: 1,
        height: content.height,
    };
    let mut state = ScrollbarState::new(content_len)
        .position(app.scroll())
        .viewport_content_length(usize::from(content.height));
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(None)
            .thumb_style(theme.muted()),
        bar,
        &mut state,
    );
}

fn onboarding_hint(app: &App) -> String {
    let sep = symbols::ui::SEPARATOR;
    if app.current_model().is_some() || !app.models_loaded {
        format!("/ for commands{sep}! for shell{sep}/help for keys")
    } else if app.models.is_empty() {
        format!("no provider connected{sep}/config to add one")
    } else {
        format!("no model selected{sep}/model to choose one")
    }
}

fn fit_cwd(cwd: &str, max: usize) -> String {
    if cwd.width() <= max {
        return cwd.to_owned();
    }
    let parts: Vec<&str> = cwd.split('/').collect();
    for i in 1..parts.len() {
        let candidate = format!("{}/{}", symbols::ui::ELLIPSIS, parts[i..].join("/"));
        if candidate.width() <= max {
            return candidate;
        }
    }
    format!(
        "{}{}",
        symbols::ui::ELLIPSIS,
        parts.last().copied().unwrap_or_default()
    )
}

fn render_header(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let row = Rect { height: 1, ..area }.inner(Margin {
        horizontal: PAD_X,
        vertical: 0,
    });
    let inner_w = usize::from(row.width);

    let ctx_span = app
        .ctx_indicator()
        .map(|(pct, _, _)| Span::styled(format!("ctx {pct:>3.0}%"), theme.meter(pct)));
    let ctx_w = ctx_span.as_ref().map_or(0, |s| s.content.width());

    let mut model_spans: Vec<Span> = Vec::new();
    if let Some(model) = app.current_model() {
        model_spans.push(Span::styled(symbols::ui::SEPARATOR, theme.muted()));
        let model_label = if app.provider_has_multiple_accounts(&model.provider) {
            format!("{}:{}/{}", model.provider, model.account, model.model)
        } else {
            format!("{}/{}", model.provider, model.model)
        };
        model_spans.push(Span::styled(model_label, theme.key()));
        if let Some(effort) = model.effort {
            model_spans.push(Span::styled(
                format!("{}{}", symbols::ui::SEPARATOR, effort),
                theme.accent(),
            ));
        }
    }
    let model_w: usize = model_spans.iter().map(|s| s.content.width()).sum();

    let cwd_max = inner_w
        .saturating_sub(model_w)
        .saturating_sub(if ctx_w > 0 { ctx_w + 2 } else { 0 });
    let cwd = fit_cwd(app.cwd(), cwd_max);

    let mut spans = vec![Span::styled(cwd.clone(), theme.muted())];
    spans.extend(model_spans);
    if let Some(ctx) = ctx_span {
        let left_w = cwd.width() + model_w;
        let pad = inner_w.saturating_sub(left_w + ctx_w);
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(ctx);
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), row);
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let inner = area.inner(Margin {
        horizontal: PAD_X,
        vertical: 0,
    });
    if app.quit_armed() {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{}c", symbols::key::CTRL), theme.hint_key()),
                Span::styled(" again to quit", theme.muted()),
            ])),
            inner,
        );
        return;
    }
    if app.is_busy() {
        let mut spans: Vec<Span> = Vec::new();
        if app.deny_armed() {
            spans.push(Span::styled("task running", theme.error()));
            spans.push(Span::styled(symbols::ui::SEPARATOR, theme.muted()));
        }
        spans.push(Span::styled(symbols::key::ESC, theme.hint_key()));
        spans.push(Span::styled(" interrupt", theme.muted()));
        if !app.agent_runs().is_empty() {
            spans.push(Span::styled(symbols::ui::SEPARATOR, theme.muted()));
            spans.push(Span::styled(symbols::key::ARROW_DOWN, theme.hint_key()));
            spans.push(Span::styled(" agents", theme.muted()));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), inner);
        return;
    }
    if app.clear_armed() {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(symbols::key::ESC, theme.hint_key()),
                Span::styled(" again to clear", theme.muted()),
            ])),
            inner,
        );
        return;
    }
    if app.transcript.items.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                onboarding_hint(app),
                theme.muted(),
            ))),
            inner,
        );
    }
}
