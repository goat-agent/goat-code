mod item;
mod render;

use std::cell::RefCell;

use goat_protocol::{TaskId, ToolCall, ToolCallId, ToolOutcome};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{highlight::Highlighter, markdown, symbols, theme::Theme};

pub(crate) use item::{Item, ShellStatus, ToolStatus, Working};
use render::{build_static_lines, hang, is_blank, queued_rows, stable_prefix_len, working_rows};

pub(crate) struct RenderCtx<'a> {
    pub theme: Theme,
    pub scroll: usize,
    pub spinner: &'static str,
    pub working: Option<&'a Working>,
    pub queued: &'a [String],
    pub hl: &'a dyn Highlighter,
    pub picker: Option<&'a ratatui_image::picker::Picker>,
}

pub(super) struct ImagePlacement {
    pub(super) item: usize,
    pub(super) start: usize,
    pub(super) rows: u16,
}

struct RenderCache {
    width: u16,
    version: u64,
    lines: Vec<Line<'static>>,
    spinner_lines: Vec<usize>,
    images: Vec<ImagePlacement>,
}

struct StreamCache {
    prefix_len: usize,
    width: u16,
    prefix_content: Vec<ratatui::text::Line<'static>>,
}

#[derive(Default)]
pub struct Transcript {
    pub(crate) items: Vec<Item>,
    streaming: Option<String>,
    version: u64,
    cache: RefCell<Option<RenderCache>>,
    stream_cache: RefCell<Option<StreamCache>>,
}

impl Transcript {
    fn bump_version(&mut self) {
        self.version = self.version.wrapping_add(1);
        *self.cache.borrow_mut() = None;
        *self.stream_cache.borrow_mut() = None;
    }

    pub fn invalidate(&mut self) {
        self.bump_version();
    }

    pub fn clear(&mut self) {
        self.bump_version();
        self.items.clear();
        self.streaming = None;
    }

    pub fn push_user(&mut self, text: impl Into<String>) {
        self.bump_version();
        self.items.push(Item::User(text.into()));
    }

    pub fn push_delta(&mut self, chunk: &str) {
        self.streaming
            .get_or_insert_with(String::new)
            .push_str(chunk);
    }

    pub fn commit_text(&mut self, text: &str) {
        self.bump_version();
        self.streaming = None;
        self.items.push(Item::Agent(text.to_owned()));
    }

    pub fn push_tool(&mut self, call: ToolCall) {
        self.bump_version();
        self.items.push(Item::Tool {
            id: call.id,
            name: call.name,
            display: call.display,
            status: ToolStatus::Running,
            image: None,
        });
    }

    pub fn finish_tool(
        &mut self,
        call_id: ToolCallId,
        mut outcome: ToolOutcome,
        picker: Option<&ratatui_image::picker::Picker>,
    ) {
        self.bump_version();
        let built = outcome
            .image
            .take()
            .map(|data| Box::new(crate::screenshot::TranscriptImage::new(data, picker)));
        for item in self.items.iter_mut().rev() {
            if let Item::Tool {
                id, status, image, ..
            } = item
                && *id == call_id
                && matches!(status, ToolStatus::Running)
            {
                *status = ToolStatus::Done(outcome);
                *image = built;
                return;
            }
        }
    }

    pub fn push_shell(&mut self, id: TaskId, command: String) {
        self.bump_version();
        self.items.push(Item::Shell {
            id,
            command,
            status: ShellStatus::Running,
        });
    }

    pub fn finish_shell(&mut self, task_id: TaskId, output: String) {
        self.bump_version();
        for item in self.items.iter_mut().rev() {
            if let Item::Shell { id, status, .. } = item
                && *id == task_id
                && matches!(status, ShellStatus::Running)
            {
                *status = ShellStatus::Done(output);
                return;
            }
        }
    }

    pub fn push_error(&mut self, text: impl Into<String>) {
        self.bump_version();
        if let Some(buffer) = self.streaming.take() {
            let text = format!("{buffer} {} stopped", symbols::ui::ELLIPSIS);
            self.items.push(Item::Agent(text));
        }
        self.items.push(Item::Error(text.into()));
    }

    pub fn discard_stream(&mut self) {
        self.bump_version();
        self.streaming = None;
    }

    pub fn push_compaction(&mut self, tokens_before: u32, tokens_after: u32) {
        self.bump_version();
        self.items.push(Item::Compaction {
            tokens_before,
            tokens_after,
        });
    }

    pub fn complete(&mut self, interrupted: bool) {
        self.bump_version();
        if interrupted {
            for item in &mut self.items {
                if let Item::Tool { status, .. } = item
                    && matches!(status, ToolStatus::Running)
                {
                    *status = ToolStatus::Done(ToolOutcome {
                        ok: false,
                        summary: None,
                        image: None,
                    });
                }
                if let Item::Shell { status, .. } = item
                    && matches!(status, ShellStatus::Running)
                {
                    *status = ShellStatus::Done(String::new());
                }
            }
        }
        if let Some(buffer) = self.streaming.take() {
            let text = if interrupted {
                format!("{buffer} … interrupted")
            } else {
                buffer
            };
            self.items.push(Item::Agent(text));
        } else if interrupted && !matches!(self.items.last(), Some(Item::Error(_))) {
            self.items.push(Item::Notice("interrupted".into()));
        }
    }

    fn ensure_cache(&self, theme: Theme, width: u16, hl: &dyn Highlighter) {
        let valid = self
            .cache
            .borrow()
            .as_ref()
            .is_some_and(|c| c.width == width && c.version == self.version);
        if valid {
            return;
        }
        let (lines, spinner_lines, images) = build_static_lines(&self.items, theme, width, hl);
        *self.cache.borrow_mut() = Some(RenderCache {
            width,
            version: self.version,
            lines,
            spinner_lines,
            images,
        });
    }

    fn streaming_rows(&self, theme: Theme, width: u16, hl: &dyn Highlighter) -> Vec<Line<'static>> {
        let Some(buffer) = &self.streaming else {
            return Vec::new();
        };
        let prefix_len = stable_prefix_len(buffer);
        let prefix_content = {
            let mut guard = self.stream_cache.borrow_mut();
            match guard.as_ref() {
                Some(cached) if cached.prefix_len == prefix_len && cached.width == width => {
                    cached.prefix_content.clone()
                }
                _ => {
                    let rendered = markdown::render(&buffer[..prefix_len], theme, hl);
                    *guard = Some(StreamCache {
                        prefix_len,
                        width,
                        prefix_content: rendered.clone(),
                    });
                    rendered
                }
            }
        };
        let mut content = prefix_content;
        let tail = markdown::render(&buffer[prefix_len..], theme, hl);
        if !content.is_empty() && !tail.is_empty() {
            content.push(Line::default());
        }
        content.extend(tail);
        while content.last().is_some_and(is_blank) {
            content.pop();
        }
        if let Some(last) = content.last_mut() {
            last.spans
                .push(Span::styled(symbols::ui::STREAM_CURSOR, theme.accent()));
        }
        hang(
            &content,
            Span::styled(symbols::marker::AGENT, theme.role_agent()),
            width,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn tail_rows(
        &self,
        theme: Theme,
        width: u16,
        hl: &dyn Highlighter,
        spinner: &'static str,
        working: Option<&Working>,
        queued: &[String],
        base_nonempty: bool,
    ) -> Vec<Line<'static>> {
        let mut tail: Vec<Line<'static>> = Vec::new();
        let streamed = self.streaming_rows(theme, width, hl);
        if !streamed.is_empty() {
            if base_nonempty {
                tail.push(Line::default());
            }
            tail.extend(streamed);
        }
        if let Some(w) = working {
            if base_nonempty || !tail.is_empty() {
                tail.push(Line::default());
            }
            tail.extend(working_rows(theme, width, spinner, w));
        }
        if !queued.is_empty() {
            if working.is_none() && (base_nonempty || !tail.is_empty()) {
                tail.push(Line::default());
            }
            tail.extend(queued_rows(theme, width, queued));
        }
        tail
    }

    pub fn content_height(
        &self,
        width: u16,
        theme: Theme,
        hl: &dyn Highlighter,
        working: Option<&Working>,
        queued: &[String],
    ) -> usize {
        self.ensure_cache(theme, width, hl);
        let base = self.cache.borrow().as_ref().map_or(0, |c| c.lines.len());
        base + self
            .tail_rows(
                theme,
                width,
                hl,
                symbols::SPINNER[0],
                working,
                queued,
                base > 0,
            )
            .len()
    }

    pub(crate) fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderCtx<'_>) {
        self.ensure_cache(ctx.theme, area.width, ctx.hl);
        let guard = self.cache.borrow();
        let Some(cache) = guard.as_ref() else {
            return;
        };
        let tail = self.tail_rows(
            ctx.theme,
            area.width,
            ctx.hl,
            ctx.spinner,
            ctx.working,
            ctx.queued,
            !cache.lines.is_empty(),
        );
        let total = cache.lines.len() + tail.len();
        let height = usize::from(area.height);
        let start = ctx.scroll.min(total.saturating_sub(height));
        let end = (start + height).min(total);
        let mut visible: Vec<Line<'static>> = Vec::with_capacity(end.saturating_sub(start));
        let static_end = end.min(cache.lines.len());
        for i in start.min(static_end)..static_end {
            let mut line = cache.lines[i].clone();
            if cache.spinner_lines.binary_search(&i).is_ok()
                && let Some(span) = line.spans.first_mut()
            {
                *span = Span::styled(ctx.spinner, ctx.theme.accent());
            }
            visible.push(line);
        }
        if end > cache.lines.len() {
            let from = start.saturating_sub(cache.lines.len());
            let to = end - cache.lines.len();
            visible.extend(tail.into_iter().take(to).skip(from));
        }
        frame.render_widget(Paragraph::new(visible), area);
        let Some(picker) = ctx.picker else {
            return;
        };
        for placement in &cache.images {
            if placement.start < start {
                continue;
            }
            let top = placement.start - start;
            let bottom = top + usize::from(placement.rows);
            if bottom > height {
                continue;
            }
            let Some(Item::Tool {
                image: Some(img), ..
            }) = self.items.get(placement.item)
            else {
                continue;
            };
            let rect = Rect {
                x: area.x,
                y: area.y + u16::try_from(top).unwrap_or(u16::MAX),
                width: area.width,
                height: placement.rows,
            };
            img.render(frame, rect, picker);
        }
    }
}

#[cfg(test)]
mod tests {
    use goat_protocol::{TaskId, ToolCall, ToolCallId, ToolOutcome};
    use ratatui::{Terminal, backend::TestBackend};

    use super::render::{
        SHELL_BLOCK_CAP, build_static_lines, format_elapsed, sanitize_shell_output, shell_rows,
        stable_prefix_len,
    };
    use super::{Item, ShellStatus, ToolStatus, Transcript, Working};
    use crate::{highlight::PlainHighlighter, markdown, symbols, theme::Theme};
    use ratatui::text::Line;

    fn call(id: u64, name: &str, input: &str) -> ToolCall {
        ToolCall {
            id: ToolCallId(id),
            name: name.to_owned(),
            display: goat_protocol::ToolDisplay::primary(input),
        }
    }

    fn ok() -> ToolOutcome {
        ToolOutcome {
            ok: true,
            summary: None,
            image: None,
        }
    }

    fn failed(summary: &str) -> ToolOutcome {
        ToolOutcome {
            ok: false,
            summary: Some(summary.to_owned()),
            image: None,
        }
    }

    fn commit(t: &mut Transcript, text: &str) {
        t.commit_text(text);
    }

    fn height(t: &Transcript, width: u16) -> usize {
        t.content_height(width, Theme::dark(), &PlainHighlighter, None, &[])
    }

    #[test]
    fn agent_text_restyles_on_theme_change() {
        let dark = Theme::dark();
        let light = Theme::light();
        let items = vec![Item::Agent("plain body".to_owned())];
        let (dark_lines, _, _) = build_static_lines(&items, dark, 80, &PlainHighlighter);
        let (light_lines, _, _) = build_static_lines(&items, light, 80, &PlainHighlighter);
        let body_fg = |lines: &[Line<'static>]| {
            lines
                .iter()
                .flat_map(|l| &l.spans)
                .find(|s| s.content.contains("plain body"))
                .map(|s| s.style.fg)
        };
        assert_eq!(body_fg(&dark_lines), Some(Some(dark.fg_color())));
        assert_eq!(body_fg(&light_lines), Some(Some(light.fg_color())));
    }

    fn buffer_row(terminal: &Terminal<TestBackend>, y: u16) -> String {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    #[test]
    fn tool_lifecycle() {
        let mut t = Transcript::default();
        t.push_tool(call(1, "Read", "src/lib.rs"));
        assert!(matches!(
            &t.items[0],
            Item::Tool {
                status: ToolStatus::Running,
                ..
            }
        ));
        t.finish_tool(ToolCallId(1), ok(), None);
        assert!(matches!(&t.items[0], Item::Tool { status: ToolStatus::Done(o), .. } if o.ok));
    }

    #[test]
    fn tool_failed_with_summary() {
        let mut t = Transcript::default();
        t.push_tool(call(2, "Bash", "cargo build"));
        t.finish_tool(ToolCallId(2), failed("error[E0308]"), None);
        if let Some(Item::Tool {
            status: ToolStatus::Done(outcome),
            ..
        }) = t.items.last()
        {
            assert!(!outcome.ok);
            assert_eq!(outcome.summary.as_deref(), Some("error[E0308]"));
        } else {
            panic!("expected done tool");
        }
    }

    #[test]
    fn agent_loop_ordering() {
        let mut t = Transcript::default();
        t.push_user("hi");
        t.push_delta("step one");
        commit(&mut t, "step one");
        t.push_tool(call(1, "Read", "src/lib.rs"));
        t.finish_tool(ToolCallId(1), ok(), None);
        t.push_delta("step two");
        commit(&mut t, "step two");

        assert!(matches!(&t.items[0], Item::User(_)));
        assert!(matches!(&t.items[1], Item::Agent(_)));
        assert!(matches!(&t.items[2], Item::Tool { .. }));
        assert!(matches!(&t.items[3], Item::Agent(_)));
    }

    #[test]
    fn complete_interrupted_clears_running_tools() {
        let mut t = Transcript::default();
        t.push_tool(call(5, "Bash", "long cmd"));
        t.complete(true);
        if let Some(Item::Tool {
            status: ToolStatus::Done(o),
            ..
        }) = t.items.first()
        {
            assert!(!o.ok);
        } else {
            panic!("expected failed tool");
        }
        assert!(
            matches!(t.items.last(), Some(Item::Notice(_))),
            "interrupt with no stream must append Notice"
        );
    }

    #[test]
    fn finish_tool_by_id_reverse_order() {
        let mut t = Transcript::default();
        t.push_tool(call(10, "Read", "a"));
        t.push_tool(call(11, "Grep", "b"));
        t.finish_tool(ToolCallId(11), ok(), None);
        t.finish_tool(ToolCallId(10), failed("err"), None);
        assert!(matches!(&t.items[0], Item::Tool { status: ToolStatus::Done(o), .. } if !o.ok));
        assert!(matches!(&t.items[1], Item::Tool { status: ToolStatus::Done(o), .. } if o.ok));
    }

    #[test]
    fn content_height_counts_streaming() {
        let mut t = Transcript::default();
        commit(&mut t, "hello world");
        let h1 = height(&t, 80);
        t.push_delta("line one\nline two\nline three\nline four");
        let h2 = height(&t, 80);
        assert!(
            h2 > h1,
            "content_height must grow while streaming is active"
        );
    }

    #[test]
    fn content_height_reserves_image_rows() {
        let mut t = Transcript::default();
        t.push_tool(call(1, "Browser", "screenshot"));
        t.finish_tool(ToolCallId(1), ok(), None);
        let without = height(&t, 80);
        if let Some(Item::Tool { image, .. }) = t.items.last_mut() {
            *image = Some(Box::new(crate::screenshot::TranscriptImage::fixed(5)));
        }
        t.bump_version();
        let with = height(&t, 80);
        assert_eq!(with, without + 5);
    }

    #[test]
    fn content_height_includes_working_line() {
        let mut t = Transcript::default();
        commit(&mut t, "hello world");
        let idle = height(&t, 80);
        let working = Working {
            elapsed: None,
            label: None,
            thinking: false,
            tokens: None,
        };
        let busy = t.content_height(80, Theme::dark(), &PlainHighlighter, Some(&working), &[]);
        assert!(
            busy > idle,
            "content_height must be larger when busy (working line)"
        );
    }

    #[test]
    fn interrupted_without_stream_pushes_notice() {
        let mut t = Transcript::default();
        t.complete(true);
        assert!(
            matches!(t.items.last(), Some(Item::Notice(_))),
            "interrupting with no stream must push a Notice item"
        );
    }

    #[test]
    fn error_commits_partial_stream_before_error_row() {
        let mut t = Transcript::default();
        t.push_delta("partial answer");
        t.push_error("boom");
        assert!(matches!(&t.items[0], Item::Agent(_)));
        assert!(matches!(&t.items[1], Item::Error(_)));
        t.complete(true);
        assert!(
            !matches!(t.items.last(), Some(Item::Notice(_))),
            "interrupted notice must be suppressed right after an error row"
        );
    }

    #[test]
    fn discard_stream_drops_partial_only() {
        let mut t = Transcript::default();
        commit(&mut t, "committed");
        t.push_delta("doomed partial");
        let before = height(&t, 80);
        t.discard_stream();
        assert!(height(&t, 80) < before);
        assert_eq!(t.items.len(), 1);
    }

    #[test]
    fn word_wrapped_bottom_is_visible_at_max_scroll() {
        let mut t = Transcript::default();
        commit(&mut t, "aaaaaaa bbbbbbb ccccccc");
        let h = height(&t, 12);
        assert_eq!(h, 3, "word wrap must yield three visual rows at width 12");
        let mut terminal = Terminal::new(TestBackend::new(12, 2)).unwrap();
        terminal
            .draw(|frame| {
                t.render(
                    frame,
                    frame.area(),
                    &super::RenderCtx {
                        theme: Theme::dark(),
                        scroll: h - 2,
                        spinner: symbols::SPINNER[0],
                        working: None,
                        queued: &[],
                        hl: &PlainHighlighter,
                        picker: None,
                    },
                );
            })
            .unwrap();
        assert!(buffer_row(&terminal, 1).contains("ccccccc"));
    }

    #[test]
    fn running_tool_renders_current_spinner_frame() {
        let mut t = Transcript::default();
        t.push_tool(call(1, "Read", "x"));
        let mut terminal = Terminal::new(TestBackend::new(20, 2)).unwrap();
        terminal
            .draw(|frame| {
                t.render(
                    frame,
                    frame.area(),
                    &super::RenderCtx {
                        theme: Theme::dark(),
                        scroll: 0,
                        spinner: symbols::SPINNER[3],
                        working: None,
                        queued: &[],
                        hl: &PlainHighlighter,
                        picker: None,
                    },
                );
            })
            .unwrap();
        assert!(buffer_row(&terminal, 0).starts_with(symbols::SPINNER[3]));
    }

    #[test]
    fn content_height_exceeds_u16_for_huge_transcripts() {
        let mut t = Transcript::default();
        t.push_user("x\n".repeat(70_000));
        assert!(height(&t, 80) > usize::from(u16::MAX));
    }

    #[test]
    fn queued_rows_render_below_working_and_cap() {
        let mut t = Transcript::default();
        commit(&mut t, "answer");
        let queued: Vec<String> = (0..5).map(|i| format!("queued {i}")).collect();
        let with_queue = t.content_height(80, Theme::dark(), &PlainHighlighter, None, &queued);
        let without = t.content_height(80, Theme::dark(), &PlainHighlighter, None, &[]);
        assert_eq!(with_queue - without, 5, "3 rows + overflow row + spacer");
    }

    #[test]
    fn format_elapsed_scales_units() {
        assert_eq!(format_elapsed(42), "42s");
        assert_eq!(format_elapsed(92), "1m32s");
        assert_eq!(format_elapsed(3_725), "1h02m");
    }

    fn line_text(line: &ratatui::text::Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.clone()).collect()
    }

    #[test]
    fn shell_lifecycle() {
        let mut t = Transcript::default();
        t.push_shell(TaskId(1), "echo hi".to_owned());
        assert!(matches!(
            &t.items[0],
            Item::Shell {
                status: ShellStatus::Running,
                ..
            }
        ));
        t.finish_shell(TaskId(1), "hi".to_owned());
        assert!(matches!(
            &t.items[0],
            Item::Shell {
                status: ShellStatus::Done(output),
                ..
            } if output == "hi"
        ));
    }

    #[test]
    fn complete_interrupted_finishes_running_shell() {
        let mut t = Transcript::default();
        t.push_shell(TaskId(2), "sleep 99".to_owned());
        t.complete(true);
        assert!(matches!(
            &t.items[0],
            Item::Shell {
                status: ShellStatus::Done(_),
                ..
            }
        ));
    }

    #[test]
    fn sanitize_strips_ansi_and_resolves_carriage_returns() {
        let lines = sanitize_shell_output("\u{1b}[31mred\u{1b}[0m\nstep1\rstep2\rdone\nlast\r\n\n");
        assert_eq!(lines, vec!["red", "done", "last"]);
    }

    #[test]
    fn shell_rows_render_command_and_output() {
        let rows = shell_rows(
            "echo hi",
            &ShellStatus::Done("hi".to_owned()),
            Theme::dark(),
            80,
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(line_text(&rows[0]), "! echo hi");
        assert_eq!(line_text(&rows[1]), "  hi");
    }

    #[test]
    fn shell_rows_caps_visual_rows_and_pins_exit_code() {
        let output = format!("{}\nexit code: 2", "x".repeat(30 * 76));
        let rows = shell_rows("badcmd", &ShellStatus::Done(output), Theme::dark(), 80);
        assert_eq!(rows.len(), 1 + SHELL_BLOCK_CAP + 2);
        assert!(line_text(&rows[1 + SHELL_BLOCK_CAP]).contains("more"));
        assert!(line_text(rows.last().unwrap()).contains("exit code: 2"));
    }

    #[test]
    fn shell_rows_show_no_output_placeholder() {
        let rows = shell_rows(
            "true",
            &ShellStatus::Done("  \n".to_owned()),
            Theme::dark(),
            80,
        );
        assert_eq!(rows.len(), 2);
        assert!(line_text(&rows[1]).contains("(no output)"));
    }

    #[test]
    fn stable_prefix_splits_outside_fences_only() {
        let text = "para one\n\npara two\n\nmore";
        let split = stable_prefix_len(text);
        assert_eq!(&text[..split], "para one\n\npara two\n\n");

        let fenced = "intro\n\n```\ncode\n\nstill code\n```\n\nafter";
        let split = stable_prefix_len(fenced);
        assert_eq!(
            &fenced[..split],
            "intro\n\n```\ncode\n\nstill code\n```\n\n"
        );

        let open_fence = "intro\n\n```\ncode\n\nstill code";
        let split = stable_prefix_len(open_fence);
        assert_eq!(
            &open_fence[..split],
            "intro\n\n",
            "must not split at a blank line inside an open fence"
        );
    }

    #[test]
    fn incremental_stream_render_matches_full_render() {
        let hl = PlainHighlighter;
        let theme = Theme::dark();
        let buffer = "# Title\n\nSome **bold** text.\n\n```rust\nfn main() {}\n```\n\nTail line";
        let split = stable_prefix_len(buffer);
        let mut incremental = markdown::render(&buffer[..split], theme, &hl);
        let tail = markdown::render(&buffer[split..], theme, &hl);
        if !incremental.is_empty() && !tail.is_empty() {
            incremental.push(ratatui::text::Line::default());
        }
        incremental.extend(tail);
        let full = markdown::render(buffer, theme, &hl);
        let render_text = |lines: &[ratatui::text::Line<'static>]| {
            lines
                .iter()
                .map(|l| {
                    l.spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(render_text(&incremental), render_text(&full));
    }
}
