use goat_worktree::WorkspaceKind;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{App, Overlay, shorten_home},
    layout::{LIST_MAX, PAD_X, SCROLL_GUTTER, format_tokens},
    overlay, symbols,
    theme::Theme,
};

#[allow(clippy::too_many_lines)]
pub fn render(frame: &mut Frame, app: &mut App) {
    let theme = app.theme();
    let full = frame.area();
    frame.render_widget(Block::new().style(theme.base()), full);

    let area = full.inner(Margin {
        horizontal: 0,
        vertical: 0,
    });

    let composer_h = app.composer_height(area.width);

    if let Overlay::ImageZoom(source) = app.overlay() {
        let [body, hint] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
        let img_area = body.inner(Margin {
            horizontal: 2,
            vertical: 1,
        });
        if let Some(picker) = app.picker.as_ref() {
            crate::screenshot::render_zoom(frame, img_area, picker, source);
        } else {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    " image preview unavailable in this terminal ",
                    theme.muted(),
                ))),
                img_area,
            );
        }
        frame.render_widget(
            Paragraph::new(overlay::hint_line(&[(symbols::key::ESC, "close")], theme)),
            hint,
        );
        return;
    }

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

    if let Overlay::Files(menu) = app.overlay() {
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
        if let Overlay::Files(menu) = app.overlay() {
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

    let preview_h = composer_preview_height(app);
    if footer_visible(app) {
        let [header, body, preview, composer, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(preview_h),
            Constraint::Length(composer_h),
            Constraint::Length(1),
        ])
        .areas(area);
        render_header(frame, header, app, theme);
        render_transcript(frame, body, app, theme);
        render_composer_preview(frame, preview, app, theme);
        app.composer().render(frame, composer, theme, true);
        render_footer(frame, footer, app, theme);
    } else {
        let [header, body, preview, composer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(preview_h),
            Constraint::Length(composer_h),
        ])
        .areas(area);
        render_header(frame, header, app, theme);
        render_transcript(frame, body, app, theme);
        render_composer_preview(frame, preview, app, theme);
        app.composer().render(frame, composer, theme, true);
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
                Some(true) => (symbols::ui::CHECK, theme.success()),
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

fn render_selection(frame: &mut Frame, app: &mut App, theme: Theme) {
    let Some(sel) = app.selection else {
        return;
    };
    if app.active_transcript().version() != app.selection_version {
        app.selection = None;
        return;
    }
    if sel.is_empty() {
        return;
    }
    let area = app.transcript_area;
    let scroll = app.scroll;
    let (start, end) = sel.bounds();
    let left = area.x.saturating_add(PAD_X);
    let right = area.x.saturating_add(area.width);
    let buf = frame.buffer_mut();
    for line in start.0..=end.0 {
        if line < scroll {
            continue;
        }
        let rel = line - scroll;
        if rel >= usize::from(area.height) {
            break;
        }
        let y = area
            .y
            .saturating_add(u16::try_from(rel).unwrap_or(u16::MAX));
        let col_lo = if line == start.0 { start.1 } else { 0 };
        let col_hi = if line == end.0 {
            end.1
        } else {
            area.width.saturating_sub(PAD_X)
        };
        let mut x = left.saturating_add(col_lo);
        let x_end = left.saturating_add(col_hi).min(right);
        while x < x_end {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(theme.selection());
            }
            x = x.saturating_add(1);
        }
    }
}

fn render_transcript(frame: &mut Frame, area: Rect, app: &mut App, theme: Theme) {
    let content = Rect {
        x: area.x,
        y: area.y,
        width: area.width.saturating_sub(SCROLL_GUTTER),
        height: area.height,
    };
    let body_width = content.width.saturating_sub(PAD_X);
    app.clamp_scroll(content.height, body_width);
    app.transcript_area = content;
    let working = app.working_state();
    let queued = app.queued_labels();
    app.transcript().render(
        frame,
        content,
        &crate::transcript::RenderCtx {
            theme,
            scroll: app.scroll(),
            left_pad: PAD_X,
            cwd: app.cwd(),
            spinner: app.spinner_frame(),
            working: working.as_ref(),
            queued: &queued,
            hl: &app.highlighter,
            picker: app.picker.as_ref(),
        },
    );
    render_selection(frame, app, theme);
    if app.follow() {
        return;
    }
    let content_len = app.content_height(body_width);
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
    let below = content_len.saturating_sub(app.scroll() + usize::from(content.height));
    if below > 0 {
        let label = format!(" {} {below} below ", symbols::ui::MORE_BELOW);
        let width = u16::try_from(label.chars().count()).unwrap_or(0);
        let hint = Rect {
            x: content
                .x
                .saturating_add(content.width.saturating_sub(width)),
            y: content.y.saturating_add(content.height.saturating_sub(1)),
            width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(label, theme.accent()))),
            hint,
        );
    }
}

const PREVIEW_IMG_ROWS: u16 = 8;
const PREVIEW_HEAD: usize = 3;
const PREVIEW_TAIL: usize = 3;

fn paste_preview_lines(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= PREVIEW_HEAD + PREVIEW_TAIL + 1 {
        return lines.iter().map(|s| (*s).to_owned()).collect();
    }
    let mut out: Vec<String> = lines[..PREVIEW_HEAD]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    out.push(format!(
        "⋮ ({} more lines)",
        lines.len() - PREVIEW_HEAD - PREVIEW_TAIL
    ));
    out.extend(
        lines[lines.len() - PREVIEW_TAIL..]
            .iter()
            .map(|s| (*s).to_owned()),
    );
    out
}

fn composer_preview_height(app: &App) -> u16 {
    match app.composer().cursor_token() {
        None => 0,
        Some(crate::composer::CursorToken::Image(_)) => PREVIEW_IMG_ROWS,
        Some(crate::composer::CursorToken::Paste(text)) => {
            u16::try_from(paste_preview_lines(text).len() + 2).unwrap_or(u16::MAX)
        }
    }
}

fn render_composer_preview(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    if area.height == 0 {
        return;
    }
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme.border_dim());
    let inner = block.inner(area);
    match app.composer().cursor_token() {
        None => {}
        Some(crate::composer::CursorToken::Image(att)) => {
            frame.render_widget(block, area);
            if let Some(picker) = app.picker.as_ref() {
                let source = goat_protocol::ToolImageData {
                    media_type: att.media_type.clone(),
                    data: att.data.clone(),
                };
                crate::screenshot::render_zoom(frame, inner, picker, &source);
            } else {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        " image preview unavailable in this terminal ",
                        theme.muted(),
                    ))),
                    inner,
                );
            }
        }
        Some(crate::composer::CursorToken::Paste(text)) => {
            frame.render_widget(block, area);
            let lines: Vec<Line> = paste_preview_lines(text)
                .into_iter()
                .map(|l| Line::from(Span::styled(l, theme.muted())))
                .collect();
            frame.render_widget(Paragraph::new(lines), inner);
        }
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

fn repo_basename(path: &std::path::Path) -> String {
    path.file_name()
        .map_or_else(|| shorten_home(path), |n| n.to_string_lossy().into_owned())
}

pub(crate) fn location_line_full(ws: &goat_worktree::Workspace) -> String {
    let repo = repo_basename(&ws.owner_root);
    match &ws.kind {
        WorkspaceKind::Managed { label } => format!("{repo}@{label}"),
        WorkspaceKind::Main | WorkspaceKind::OtherWorktree => {
            if ws.git_branch.is_empty() {
                repo
            } else {
                format!("{repo}:{}", ws.git_branch)
            }
        }
    }
}

fn fit_location_line(ws: &goat_worktree::Workspace, max: usize) -> String {
    let full = location_line_full(ws);
    if full.width() <= max {
        return full;
    }
    let repo = repo_basename(&ws.owner_root);
    match &ws.kind {
        WorkspaceKind::Managed { label } => {
            let tail = format!("@{label}");
            let repo_max = max.saturating_sub(tail.width());
            format!("{}{tail}", fit_cwd(&repo, repo_max))
        }
        WorkspaceKind::Main | WorkspaceKind::OtherWorktree => {
            if ws.git_branch.is_empty() {
                return fit_cwd(&repo, max);
            }
            let branch_w = ws.git_branch.width() + 1;
            let short_repo = fit_cwd(&repo, max.saturating_sub(branch_w));
            format!("{short_repo}:{}", ws.git_branch)
        }
    }
}

fn workspace_location_spans(
    ws: &goat_worktree::Workspace,
    theme: Theme,
) -> (Vec<Span<'static>>, usize) {
    let repo = repo_basename(&ws.owner_root);
    let mut spans = Vec::new();
    let mut width = 0;
    match &ws.kind {
        WorkspaceKind::Managed { label } => {
            width += repo.width();
            spans.push(Span::styled(repo.clone(), theme.muted()));
            width += 1;
            spans.push(Span::styled("@", theme.muted()));
            width += label.width();
            spans.push(Span::styled(label.clone(), theme.text()));
        }
        WorkspaceKind::Main | WorkspaceKind::OtherWorktree => {
            width += repo.width();
            spans.push(Span::styled(repo, theme.muted()));
            if !ws.git_branch.is_empty() {
                width += 1;
                spans.push(Span::styled(":", theme.muted()));
                let branch = ws.git_branch.clone();
                width += branch.width();
                spans.push(Span::styled(branch, theme.text()));
            }
        }
    }
    (spans, width)
}

fn fit_workspace_location_spans(
    ws: &goat_worktree::Workspace,
    max: usize,
    theme: Theme,
) -> (Vec<Span<'static>>, usize) {
    let (spans, width) = workspace_location_spans(ws, theme);
    if width <= max {
        return (spans, width);
    }
    let fitted = fit_location_line(ws, max);
    let w = fitted.width();
    (vec![Span::styled(fitted, theme.muted())], w)
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

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn format_ctx_status(used: u64, window: u32) -> (String, f32) {
    let pct = if window == 0 {
        0.0
    } else {
        (used as f64 / f64::from(window) * 100.0).min(100.0) as f32
    };
    let label = format!(
        "{}/{}",
        format_tokens(used),
        format_tokens(u64::from(window))
    );
    (label, pct)
}

fn format_rate_status(windows: &[(String, f32)]) -> Vec<(String, f32)> {
    windows
        .iter()
        .map(|(label, pct)| (format!("{label} {pct:.0}%"), *pct))
        .collect()
}

fn ctx_label(app: &App) -> Option<(String, f32)> {
    app.ctx_indicator()
        .map(|(_, used, window)| format_ctx_status(used, window))
}

fn rate_labels(app: &App) -> Vec<(String, f32)> {
    app.rate_limit_indicator()
        .map(|windows| format_rate_status(&windows))
        .unwrap_or_default()
}

pub(crate) fn window_label(window_count: usize) -> Option<String> {
    (window_count > 1).then(|| format!("\u{29c9} {window_count}"))
}

fn render_header(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let row = Rect { height: 1, ..area }.inner(Margin {
        horizontal: PAD_X,
        vertical: 0,
    });
    let inner_w = usize::from(row.width);

    let model = model_label(app);
    let ctx = ctx_label(app);
    let rates = rate_labels(app);
    let windows = window_label(app.window_count);
    let model_w = model.as_ref().map_or(0, |label| label.width());
    let ctx_w = ctx.as_ref().map_or(0, |(label, _)| label.width());
    let rates_w = rates
        .iter()
        .map(|(label, _)| 2 + label.width())
        .sum::<usize>();
    let windows_w = windows.as_ref().map_or(0, |label| label.width());
    let status_gap = (usize::from(model.is_some())
        + usize::from(ctx.is_some())
        + usize::from(windows.is_some()))
        * 2;
    let status_w = model_w + ctx_w + rates_w + windows_w + status_gap;
    let left_max = inner_w.saturating_sub(status_w);
    let (mut spans, left_w) = if let Some(ws) = app.workspace_snapshot() {
        fit_workspace_location_spans(ws, left_max, theme)
    } else {
        let cwd = fit_cwd(app.cwd(), left_max);
        let w = cwd.width();
        (vec![Span::styled(cwd, theme.muted())], w)
    };
    let pad = inner_w.saturating_sub(left_w + status_w);
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(pad)));
    }
    if let Some(label) = windows {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(label, theme.muted()));
    }
    if let Some(label) = model {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(label, theme.key()));
    }
    if let Some((label, pct)) = ctx {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(label, theme.meter(pct)));
    }
    for (label, pct) in rates {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(label, theme.meter(pct)));
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
        spans.push(Span::styled(symbols::key::ESC, theme.hint_key()));
        spans.push(Span::styled(" interrupt", theme.muted()));
        if !app.queued.is_empty() {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(symbols::key::BACKSPACE, theme.hint_key()));
            spans.push(Span::styled(" edit queued", theme.muted()));
        }
        if !app.agent_runs().is_empty() {
            spans.push(Span::raw("  "));
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

    use super::{format_ctx_status, format_rate_status, model_status_label};
    use goat_worktree::WorkspaceKind;

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

    #[test]
    fn window_label_hidden_for_single_window() {
        assert_eq!(super::window_label(0), None);
        assert_eq!(super::window_label(1), None);
    }

    #[test]
    fn window_label_shown_for_multiple_windows() {
        assert_eq!(super::window_label(2), Some("\u{29c9} 2".to_owned()));
        assert_eq!(super::window_label(5), Some("\u{29c9} 5".to_owned()));
    }

    #[test]
    fn location_line_main() {
        let ws = goat_worktree::Workspace {
            owner_root: std::path::PathBuf::from("/x/goat-code"),
            repo_root: std::path::PathBuf::from("/x/goat-code"),
            git_branch: "main".to_owned(),
            kind: WorkspaceKind::Main,
        };
        assert_eq!(super::location_line_full(&ws), "goat-code:main");
    }

    #[test]
    fn location_line_managed() {
        let ws = goat_worktree::Workspace {
            owner_root: std::path::PathBuf::from("/x/goat-code"),
            repo_root: std::path::PathBuf::from("/x/goat-code/.goat/worktrees/plan"),
            git_branch: "worktree-plan".to_owned(),
            kind: WorkspaceKind::Managed {
                label: "plan".to_owned(),
            },
        };
        assert_eq!(super::location_line_full(&ws), "goat-code@plan");
    }

    #[test]
    fn format_ctx_status_uses_token_fraction() {
        let (label, pct) = format_ctx_status(45_000, 128_000);
        assert_eq!(label, "45k/128k");
        assert!((pct - 35.15625).abs() < f32::EPSILON);
    }

    #[test]
    fn format_rate_status_maps_window_labels() {
        let windows = vec![("5h".to_owned(), 42.0), ("weekly".to_owned(), 18.0)];
        let labels = format_rate_status(&windows);
        assert_eq!(
            labels,
            vec![("5h 42%".to_owned(), 42.0), ("weekly 18%".to_owned(), 18.0)]
        );
    }

    #[test]
    fn format_rate_status_empty() {
        assert!(format_rate_status(&[]).is_empty());
    }
}
