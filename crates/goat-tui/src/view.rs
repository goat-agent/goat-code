use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    text::{Line, Span},
    widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{App, Overlay, PlanFocus, PlanOverlay},
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

    if let Overlay::Plan(_) = app.overlay() {
        let panel_h = plan_desired_height(area.height);
        let [header, transcript_area, _panel] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(panel_h),
        ])
        .areas(area);
        render_header(frame, header, app, theme);
        render_transcript(frame, transcript_area, app, theme);
        if let Overlay::Plan(plan) = app.overlay() {
            render_plan_overlay(frame, area, plan, theme);
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
        app.composer()
            .render(frame, composer_area, theme, true, app.plan_prompt_active());
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
            app.composer()
                .render(frame, composer, theme, false, app.plan_prompt_active());
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
        app.composer()
            .render(frame, composer, theme, false, app.plan_prompt_active());
        render_toasts(frame, area, app, theme);
        return;
    }

    if footer_visible(app) {
        let [header, body, composer, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(composer_h),
            Constraint::Length(1),
        ])
        .areas(area);
        render_header(frame, header, app, theme);
        render_transcript(frame, body, app, theme);
        app.composer()
            .render(frame, composer, theme, true, app.plan_prompt_active());
        render_footer(frame, footer, app, theme);
    } else {
        let [header, body, composer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(composer_h),
        ])
        .areas(area);
        render_header(frame, header, app, theme);
        render_transcript(frame, body, app, theme);
        app.composer()
            .render(frame, composer, theme, true, app.plan_prompt_active());
    }
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

fn footer_visible(app: &App) -> bool {
    app.quit_armed() || app.is_busy() || app.clear_armed()
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
    let queued = app.queued_labels();
    app.transcript().render(
        frame,
        content,
        &crate::transcript::RenderCtx {
            theme,
            scroll: app.scroll(),
            spinner: app.spinner_frame(),
            working: working.as_ref(),
            queued: &queued,
            hl: &app.highlighter,
            picker: app.picker.as_ref(),
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

pub(crate) fn model_status_label(
    model: &goat_protocol::ModelTarget,
    multiple_accounts: bool,
) -> String {
    let mut label = if multiple_accounts {
        format!("{}:{}/{}", model.provider, model.account, model.model)
    } else {
        format!("{}/{}", model.provider, model.model)
    };
    if let Some(effort) = model.effort {
        label.push(':');
        label.push_str(effort.as_str());
    }
    label
}

fn model_label(app: &App) -> Option<String> {
    let model = app.current_model()?;
    Some(model_status_label(
        model,
        app.provider_has_multiple_accounts(&model.provider),
    ))
}

fn ctx_label(app: &App) -> Option<(String, f32)> {
    app.ctx_indicator()
        .map(|(pct, _, _)| (format!("ctx {pct:.0}%"), pct))
}

fn render_header(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let row = Rect { height: 1, ..area }.inner(Margin {
        horizontal: PAD_X,
        vertical: 0,
    });
    let inner_w = usize::from(row.width);

    let model = model_label(app);
    let ctx = ctx_label(app);
    let model_w = model.as_ref().map_or(0, |label| label.width());
    let ctx_w = ctx.as_ref().map_or(0, |(label, _)| label.width());
    let status_gap = usize::from(model.is_some()) * 2 + usize::from(ctx.is_some()) * 2;
    let status_w = model_w + ctx_w + status_gap;
    let cwd = fit_cwd(app.cwd(), inner_w.saturating_sub(status_w));

    let mut spans: Vec<Span> = vec![Span::styled(cwd.clone(), theme.muted())];
    let left_w = cwd.width();
    let pad = inner_w.saturating_sub(left_w + status_w);
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(pad)));
    }
    if let Some(label) = model {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(label, theme.key()));
    }
    if let Some((label, pct)) = ctx {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(label, theme.meter(pct)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), row);
}

fn plan_desired_height(area_height: u16) -> u16 {
    let max = area_height.saturating_sub(3).max(9);
    let preferred = (area_height / 3).max(9);
    preferred.min(max)
}

fn render_plan_overlay(frame: &mut Frame, area: Rect, plan: &PlanOverlay, theme: Theme) {
    let panel_h = plan_desired_height(area.height);
    let [_, outer] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(panel_h)]).areas(area);
    let Some(inner) = overlay::overlay_frame(frame, outer, theme) else {
        return;
    };
    let [title, preview, actions, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .areas(inner);
    render_plan_title(frame, title, plan, theme);
    render_plan_preview(frame, preview, plan, theme);
    if let Some(feedback) = &plan.feedback {
        render_plan_feedback(frame, actions, feedback, theme);
        render_plan_feedback_hint(frame, hint, theme);
    } else {
        render_plan_choices(frame, actions, plan.focus, theme);
        render_plan_choice_hint(frame, hint, theme);
    }
}

fn render_plan_title(frame: &mut Frame, area: Rect, plan: &PlanOverlay, theme: Theme) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" plan", theme.plan()),
            Span::styled(format!("  {}", plan.path), theme.muted()),
        ])),
        area,
    );
}

fn render_plan_preview(frame: &mut Frame, area: Rect, plan: &PlanOverlay, theme: Theme) {
    let para = Paragraph::new(plan.plan.as_str())
        .wrap(Wrap { trim: false })
        .scroll((plan.scroll, 0))
        .style(theme.base());
    frame.render_widget(para, area);
}

fn render_plan_choices(frame: &mut Frame, area: Rect, focus: PlanFocus, theme: Theme) {
    let width = usize::from(area.width);
    let lines = vec![
        overlay::selection_row(
            theme,
            focus == PlanFocus::Approve,
            width,
            vec![Span::styled("Approve plan", theme.base())],
            Some(Span::styled("implement this plan", theme.muted())),
        ),
        overlay::selection_row(
            theme,
            focus == PlanFocus::Reject,
            width,
            vec![Span::styled("Request changes", theme.base())],
            Some(Span::styled("send feedback", theme.muted())),
        ),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_plan_feedback(frame: &mut Frame, area: Rect, feedback: &str, theme: Theme) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" feedback: ", theme.muted()),
            Span::styled(feedback.to_owned(), theme.base()),
            Span::styled(symbols::ui::STREAM_CURSOR, theme.accent()),
        ])),
        area,
    );
    let col = 11 + UnicodeWidthStr::width(feedback);
    let x = area.x + u16::try_from(col).unwrap_or(u16::MAX);
    frame.set_cursor_position((x.min(area.right().saturating_sub(1)), area.y));
}

fn render_plan_choice_hint(frame: &mut Frame, area: Rect, theme: Theme) {
    frame.render_widget(
        Paragraph::new(overlay::hint_line(
            &[
                (symbols::key::ARROWS_UPDOWN, "choose"),
                ("pgup/pgdn", "scroll"),
                (symbols::key::ENTER, "select"),
                (symbols::key::ESC, "dismiss"),
            ],
            theme,
        )),
        area,
    );
}

fn render_plan_feedback_hint(frame: &mut Frame, area: Rect, theme: Theme) {
    frame.render_widget(
        Paragraph::new(overlay::hint_line(
            &[
                (symbols::key::ENTER, "send"),
                (symbols::key::ESC, "back"),
                ("pgup/pgdn", "scroll"),
            ],
            theme,
        )),
        area,
    );
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
        spans.push(Span::styled(symbols::key::ESC, theme.hint_key()));
        spans.push(Span::styled(" interrupt", theme.muted()));
        if !app.queued.is_empty() {
            spans.push(Span::styled(symbols::ui::SEPARATOR, theme.muted()));
            spans.push(Span::styled(symbols::key::BACKSPACE, theme.hint_key()));
            spans.push(Span::styled(" edit queued", theme.muted()));
        }
        if !app.agent_runs().is_empty() {
            spans.push(Span::styled(symbols::ui::SEPARATOR, theme.muted()));
            spans.push(Span::styled(symbols::key::ARROW_DOWN, theme.hint_key()));
            spans.push(Span::styled(" agents", theme.muted()));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), inner);
    } else if app.clear_armed() {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(symbols::key::ESC, theme.hint_key()),
                Span::styled(" again to clear", theme.muted()),
            ])),
            inner,
        );
    }
}

#[cfg(test)]
mod tests {
    use goat_protocol::{Effort, ModelTarget};

    use super::model_status_label;

    fn target(effort: Option<Effort>) -> ModelTarget {
        ModelTarget {
            provider: "anthropic".to_owned(),
            account: "work".to_owned(),
            model: "claude-sonnet-4".to_owned(),
            effort,
        }
    }

    #[test]
    fn model_status_label_omits_single_account_profile() {
        assert_eq!(
            model_status_label(&target(Some(Effort::High)), false),
            "anthropic/claude-sonnet-4:high"
        );
    }

    #[test]
    fn model_status_label_includes_profile_for_multiple_accounts() {
        assert_eq!(
            model_status_label(&target(Some(Effort::Medium)), true),
            "anthropic:work/claude-sonnet-4:medium"
        );
    }

    #[test]
    fn model_status_label_omits_missing_effort() {
        assert_eq!(
            model_status_label(&target(None), true),
            "anthropic:work/claude-sonnet-4"
        );
    }
}
