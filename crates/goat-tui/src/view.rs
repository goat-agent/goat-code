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

    if let Overlay::Ask(picker, _) = app.overlay() {
        let panel_h = picker
            .desired_height()
            .min(area.height.saturating_sub(2))
            .max(3);
        let [header, transcript_area, _panel] = Layout::vertical([
            Constraint::Length(1),
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
            Constraint::Length(1),
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
        | Overlay::Usage => {
            let [header, body, composer] = Layout::vertical([
                Constraint::Length(1),
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
        render_transcript(frame, body, app, theme);
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

const PAD_LEFT: u16 = 1;
const GUTTER: u16 = 2;

fn render_transcript(frame: &mut Frame, area: Rect, app: &mut App, theme: Theme) {
    let content = Rect {
        x: area.x + PAD_LEFT,
        y: area.y,
        width: area.width.saturating_sub(PAD_LEFT + GUTTER),
        height: area.height,
    };
    app.clamp_scroll(content.height, content.width);
    app.transcript()
        .render(frame, content, theme, app.scroll(), app.spinner_frame());
    if app.follow() {
        return;
    }
    let content_len = app.content_height(content.width);
    if content_len <= content.height {
        return;
    }
    let bar = Rect {
        x: area.x + area.width.saturating_sub(1),
        y: area.y,
        width: 1,
        height: content.height,
    };
    let mut state = ScrollbarState::new(content_len as usize)
        .position(app.scroll() as usize)
        .viewport_content_length(content.height as usize);
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

fn ctx_color(pct: f32, theme: Theme) -> ratatui::style::Style {
    if pct >= 90.0 {
        theme.error()
    } else if pct >= 70.0 {
        theme.role_tool()
    } else {
        theme.muted()
    }
}

#[allow(clippy::cast_precision_loss)]
fn format_k(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.0}k", (n as f64 / 1_000.0).round())
    } else {
        format!("{n}")
    }
}

fn render_header(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let mut left_spans = vec![Span::styled(format!(" {}", app.cwd()), theme.muted())];
    if let Some(model) = app.current_model() {
        left_spans.push(Span::styled("  \u{00b7}  ", theme.muted()));
        let model_label = if app.provider_has_multiple_accounts(&model.provider) {
            format!("{}:{}/{}", model.provider, model.account, model.model)
        } else {
            format!("{}/{}", model.provider, model.model)
        };
        left_spans.push(Span::styled(model_label, theme.key()));
        if let Some(effort) = model.effort {
            left_spans.push(Span::styled(
                format!("{}{}", symbols::ui::SEPARATOR, effort),
                theme.accent(),
            ));
        }
    }

    let mut right_spans: Vec<Span> = Vec::new();
    if let Some((pct, used, window)) = app.ctx_indicator() {
        let color = ctx_color(pct, theme);
        let area_w = usize::from(area.width);
        let left_w: usize = left_spans.iter().map(|s| s.content.len()).sum();
        let sep = symbols::ui::MIDDOT;
        let fused = format_k(used);
        let fwin = format_k(u64::from(window));
        let short_text = format!(" ctx {pct:>3.0}%");
        let long_text = format!(" ctx {pct:>3.0}% {sep} {fused}/{fwin}");
        let ctx_str = if area_w.saturating_sub(left_w) >= long_text.len() + 2 {
            long_text
        } else {
            short_text
        };
        let pad = area_w
            .saturating_sub(left_w + ctx_str.len())
            .saturating_sub(1);
        right_spans.push(Span::raw(" ".repeat(pad)));
        right_spans.push(Span::styled(ctx_str, color));
    }

    let mut spans = left_spans;
    spans.extend(right_spans);
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
