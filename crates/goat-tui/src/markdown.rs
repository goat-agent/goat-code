use pulldown_cmark::{Alignment, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};

use crate::{highlight::Highlighter, symbols, theme::Theme};

#[allow(clippy::too_many_lines)]
pub fn render(md: &str, theme: Theme, hl: &dyn Highlighter) -> Vec<Line<'static>> {
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut bold = false;
    let mut strikethrough = false;
    let mut in_code_block = false;
    let mut in_blockquote = false;
    let mut code_lang = String::new();
    let mut code_buf = String::new();
    let mut list_stack: Vec<Option<u64>> = Vec::new();
    let mut list_item_index: Vec<u64> = Vec::new();
    let mut link_url: Option<String> = None;
    let mut link_text_start: usize = 0;
    let mut col_alignments: Vec<Alignment> = Vec::new();
    let mut table_headers: Vec<Vec<Span<'static>>> = Vec::new();
    let mut table_rows: Vec<Vec<Vec<Span<'static>>>> = Vec::new();
    let mut current_row: Vec<Vec<Span<'static>>> = Vec::new();
    let mut current_cell: Vec<Span<'static>> = Vec::new();
    let mut in_table = false;
    let mut in_thead = false;
    let mut col_idx: usize = 0;

    for event in Parser::new_ext(md, opts) {
        match event {
            Event::Start(Tag::Strong) => bold = true,
            Event::End(TagEnd::Strong) => bold = false,

            Event::Start(Tag::Strikethrough) => strikethrough = true,
            Event::End(TagEnd::Strikethrough) => strikethrough = false,

            Event::Start(Tag::Heading { .. }) => {
                flush_line(&mut current_spans, &mut lines);
            }
            Event::End(TagEnd::Heading(level)) => {
                flush_line(&mut current_spans, &mut lines);
                let heading_style = heading_style(level, theme);
                if let Some(last) = lines.last_mut() {
                    let mut new_spans = Vec::new();
                    for span in last.spans.drain(..) {
                        new_spans.push(Span::styled(span.content, heading_style));
                    }
                    *last = Line::from(new_spans);
                }
                lines.push(Line::default());
            }

            Event::End(TagEnd::Paragraph) => {
                flush_line(&mut current_spans, &mut lines);
                lines.push(Line::default());
            }

            Event::Start(Tag::BlockQuote(_)) => {
                flush_line(&mut current_spans, &mut lines);
                in_blockquote = true;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush_line_blockquote(&mut current_spans, &mut lines, theme);
                in_blockquote = false;
            }

            Event::Start(Tag::List(start)) => {
                list_stack.push(start);
                list_item_index.push(start.unwrap_or(1));
            }
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
                list_item_index.pop();
            }
            Event::Start(Tag::Item) => {
                if in_blockquote {
                    flush_line_blockquote(&mut current_spans, &mut lines, theme);
                } else {
                    flush_line(&mut current_spans, &mut lines);
                }
                let bullet = next_bullet(&list_stack, &mut list_item_index);
                current_spans.push(Span::styled(bullet, theme.muted()));
            }

            Event::Start(Tag::CodeBlock(kind)) => {
                in_code_block = true;
                code_lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(lang) => lang.to_string(),
                    pulldown_cmark::CodeBlockKind::Indented => String::new(),
                };
                code_buf.clear();
                flush_line(&mut current_spans, &mut lines);
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                render_code_block(
                    code_buf.trim_end_matches('\n'),
                    &code_lang,
                    theme,
                    hl,
                    &mut lines,
                );
                lines.push(Line::default());
                code_lang.clear();
            }

            Event::Start(Tag::Link { dest_url, .. }) => {
                link_url = Some(dest_url.to_string());
                link_text_start = current_spans.len();
            }
            Event::End(TagEnd::Link) => {
                if let Some(url) = link_url.take() {
                    let link_spans = &mut current_spans[link_text_start..];
                    for span in link_spans.iter_mut() {
                        span.style = span
                            .style
                            .fg(theme.accent_color())
                            .add_modifier(Modifier::UNDERLINED);
                    }
                    let link_text: String = current_spans[link_text_start..]
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect();
                    if !link_text.is_empty() && url != link_text {
                        current_spans.push(Span::styled(format!(" ({url})"), theme.muted()));
                    }
                }
                link_text_start = 0;
            }

            Event::Start(Tag::Table(alignments)) => {
                in_table = true;
                col_alignments.clone_from(&alignments);
                table_headers = vec![Vec::new(); col_alignments.len()];
                table_rows.clear();
                flush_line(&mut current_spans, &mut lines);
            }
            Event::End(TagEnd::Table) => {
                in_table = false;
                render_table(&table_headers, &table_rows, theme, &mut lines);
                lines.push(Line::default());
                table_headers.clear();
                table_rows.clear();
                col_alignments.clear();
            }
            Event::Start(Tag::TableHead) => {
                in_thead = true;
            }
            Event::End(TagEnd::TableHead) => {
                in_thead = false;
            }
            Event::Start(Tag::TableRow) => {
                if !in_thead {
                    current_row = Vec::new();
                }
                col_idx = 0;
            }
            Event::End(TagEnd::TableRow) if !in_thead => {
                table_rows.push(current_row.clone());
                current_row.clear();
            }
            Event::Start(Tag::TableCell) => {
                current_cell = Vec::new();
            }
            Event::End(TagEnd::TableCell) => {
                if in_thead {
                    if col_idx < table_headers.len() {
                        table_headers[col_idx].clone_from(&current_cell);
                    }
                } else {
                    current_row.push(current_cell.clone());
                }
                current_cell.clear();
                col_idx += 1;
            }

            Event::Code(text) => {
                let span = Span::styled(text.to_string(), theme.inline_code());
                if in_table {
                    current_cell.push(span);
                } else {
                    current_spans.push(span);
                }
            }

            Event::Text(text) => {
                if in_code_block {
                    code_buf.push_str(&text);
                } else {
                    let style = text_style(bold, strikethrough, theme);
                    let segments: Vec<&str> = text.split('\n').collect();
                    for (i, segment) in segments.iter().enumerate() {
                        if i > 0 {
                            if in_blockquote {
                                flush_line_blockquote(&mut current_spans, &mut lines, theme);
                            } else {
                                flush_line(&mut current_spans, &mut lines);
                            }
                        }
                        if !segment.is_empty() {
                            let span = Span::styled(segment.to_string(), style);
                            if in_table {
                                current_cell.push(span);
                            } else {
                                current_spans.push(span);
                            }
                        }
                    }
                }
            }

            Event::SoftBreak if !in_table => {
                current_spans.push(Span::raw(" "));
            }

            Event::End(TagEnd::Item) | Event::HardBreak => {
                if in_blockquote {
                    flush_line_blockquote(&mut current_spans, &mut lines, theme);
                } else {
                    flush_line(&mut current_spans, &mut lines);
                }
            }

            Event::Rule => {
                flush_line(&mut current_spans, &mut lines);
                lines.push(Line::from(Span::styled(symbols::ui::RULE, theme.muted())));
                lines.push(Line::default());
            }

            _ => {}
        }
    }

    flush_line(&mut current_spans, &mut lines);

    while lines.last().is_some_and(|l: &Line| l.spans.is_empty()) {
        lines.pop();
    }

    lines
}

fn text_style(bold: bool, strikethrough: bool, theme: Theme) -> ratatui::style::Style {
    let mut style = theme.base();
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if strikethrough {
        style = style.add_modifier(Modifier::CROSSED_OUT);
    }
    style
}

fn heading_style(level: HeadingLevel, theme: Theme) -> ratatui::style::Style {
    match level {
        HeadingLevel::H1 => theme.accent().add_modifier(Modifier::BOLD),
        HeadingLevel::H2 => theme.accent(),
        _ => theme.key(),
    }
}

fn flush_line(spans: &mut Vec<Span<'static>>, lines: &mut Vec<Line<'static>>) {
    if !spans.is_empty() {
        lines.push(Line::from(std::mem::take(spans)));
    }
}

fn flush_line_blockquote(
    spans: &mut Vec<Span<'static>>,
    lines: &mut Vec<Line<'static>>,
    theme: Theme,
) {
    let mut row_spans = vec![Span::styled(symbols::ui::QUOTE_GUTTER, theme.muted())];
    row_spans.extend(
        std::mem::take(spans)
            .into_iter()
            .map(|s| Span::styled(s.content, theme.muted())),
    );
    lines.push(Line::from(row_spans));
}

fn render_code_block(
    code: &str,
    lang: &str,
    theme: Theme,
    hl: &dyn Highlighter,
    lines: &mut Vec<Line<'static>>,
) {
    let highlighted = hl.highlight(lang, code, theme);
    for (i, hl_line) in highlighted.into_iter().enumerate() {
        let mut spans = vec![Span::styled(symbols::ui::CODE_GUTTER, theme.muted())];
        spans.extend(
            hl_line
                .spans
                .into_iter()
                .map(|s| Span::styled(s.content.into_owned(), s.style)),
        );
        if i == 0 && !lang.is_empty() {
            spans.push(Span::styled(format!("  {lang}"), theme.muted()));
        }
        lines.push(Line::from(spans));
    }
}

fn render_table(
    headers: &[Vec<Span<'static>>],
    rows: &[Vec<Vec<Span<'static>>>],
    theme: Theme,
    lines: &mut Vec<Line<'static>>,
) {
    if headers.is_empty() {
        return;
    }
    let col_count = headers.len();

    let col_widths: Vec<usize> = (0..col_count)
        .map(|col| {
            let header_w = span_text_width(&headers[col]);
            let max_row_w = rows
                .iter()
                .filter_map(|row| row.get(col))
                .map(|cell| span_text_width(cell))
                .max()
                .unwrap_or(0);
            header_w.max(max_row_w).max(1)
        })
        .collect();

    lines.push(build_table_row(headers.iter(), &col_widths, theme, true));

    let sep: String = col_widths
        .iter()
        .map(|&w| "─".repeat(w + 2))
        .collect::<Vec<_>>()
        .join(" ");
    lines.push(Line::from(Span::styled(sep, theme.muted())));

    for row in rows {
        lines.push(build_table_row(row.iter(), &col_widths, theme, false));
    }
}

fn build_table_row<'a, I>(
    cells: I,
    col_widths: &[usize],
    theme: Theme,
    header: bool,
) -> Line<'static>
where
    I: Iterator<Item = &'a Vec<Span<'static>>>,
{
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (col, cell) in cells.enumerate() {
        if col > 0 {
            spans.push(Span::styled("  ", theme.muted()));
        }
        let w = col_widths.get(col).copied().unwrap_or(1);
        let text_w = span_text_width(cell);
        let pad = w.saturating_sub(text_w);
        for s in cell {
            let style = if header {
                s.style.add_modifier(Modifier::BOLD)
            } else {
                s.style
            };
            spans.push(Span::styled(s.content.clone(), style));
        }
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
    }
    Line::from(spans)
}

fn span_text_width(spans: &[Span<'static>]) -> usize {
    use unicode_width::UnicodeWidthStr;
    spans.iter().map(|s| s.content.width()).sum()
}

fn next_bullet(list_stack: &[Option<u64>], list_item_index: &mut [u64]) -> String {
    let depth = list_stack.len().saturating_sub(1);
    let indent = "  ".repeat(depth);
    let marker = match list_stack.last() {
        Some(Some(_)) => {
            let idx = list_item_index.last_mut().map_or(1, |i| {
                let v = *i;
                *i += 1;
                v
            });
            format!("{idx}. ")
        }
        Some(None) | None => symbols::ui::BULLET.to_owned(),
    };
    format!("{indent}{marker}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::highlight::PlainHighlighter;
    use crate::theme::Theme;

    fn render_plain(md: &str) -> Vec<Line<'static>> {
        render(md, Theme::dark(), &PlainHighlighter)
    }

    #[test]
    fn bold_text_has_bold_modifier() {
        let lines = render_plain("**hello**");
        let spans: Vec<_> = lines.iter().flat_map(|l| &l.spans).collect();
        assert!(
            spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::BOLD))
        );
    }

    #[test]
    fn strikethrough_has_crossed_out() {
        let lines = render_plain("~~old~~");
        let spans: Vec<_> = lines.iter().flat_map(|l| &l.spans).collect();
        assert!(
            spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::CROSSED_OUT))
        );
    }

    #[test]
    fn inline_code_uses_accent() {
        let theme = Theme::dark();
        let lines = render("use `foo` here", theme, &PlainHighlighter);
        let spans: Vec<_> = lines.iter().flat_map(|l| &l.spans).collect();
        let code_span = spans
            .iter()
            .find(|s| s.content.as_ref() == "foo")
            .expect("inline code span without padding");
        assert_eq!(code_span.style.fg, Some(theme.accent_color()));
        assert_eq!(code_span.style.bg, None);
    }

    #[test]
    fn code_block_has_bar_prefix() {
        let lines = render_plain("```rust\nfn main() {}\n```");
        assert!(
            lines
                .iter()
                .any(|l| { l.spans.first().is_some_and(|s| s.content.contains('│')) })
        );
    }

    #[test]
    fn unordered_list_renders_bullets() {
        let lines = render_plain("- one\n- two");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains('•'));
    }

    #[test]
    fn ordered_list_renders_numbers() {
        let lines = render_plain("1. first\n2. second");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("1.") || text.contains("2."));
    }

    #[test]
    fn heading_h1_uses_bold_accent_no_hash() {
        let theme = Theme::dark();
        let lines = render("# Title", theme, &PlainHighlighter);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(!text.contains('#'));
        let spans: Vec<_> = lines.iter().flat_map(|l| &l.spans).collect();
        assert!(spans.iter().any(|s| {
            s.style.add_modifier.contains(Modifier::BOLD)
                && s.style.fg == Some(theme.accent_color())
        }));
    }

    #[test]
    fn heading_h2_uses_accent_no_hash() {
        let theme = Theme::dark();
        let lines = render("## Section", theme, &PlainHighlighter);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(!text.contains("##"));
        let spans: Vec<_> = lines.iter().flat_map(|l| &l.spans).collect();
        assert!(
            spans
                .iter()
                .any(|s| s.style.fg == Some(theme.accent_color()))
        );
    }

    #[test]
    fn horizontal_rule_emits_rule_glyph() {
        let lines = render_plain("---");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains('─'));
    }

    #[test]
    fn blockquote_has_gutter() {
        let lines = render_plain("> quoted text");
        assert!(
            lines
                .iter()
                .any(|l| l.spans.first().is_some_and(|s| s.content.contains('▎')))
        );
    }

    #[test]
    fn link_text_has_underline() {
        let theme = Theme::dark();
        let lines = render("[docs](https://example.com)", theme, &PlainHighlighter);
        let spans: Vec<_> = lines.iter().flat_map(|l| &l.spans).collect();
        assert!(
            spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::UNDERLINED))
        );
    }

    #[test]
    fn table_renders_header_bold() {
        let lines = render_plain("| a | b |\n|---|---|\n| 1 | 2 |");
        let spans: Vec<_> = lines.iter().flat_map(|l| &l.spans).collect();
        assert!(
            spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::BOLD))
        );
    }

    #[test]
    fn plain_text_passes_through() {
        let lines = render_plain("hello world");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("hello world"));
    }
}
