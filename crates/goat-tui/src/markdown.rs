use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};

use crate::{highlight::Highlighter, theme::Theme};

pub fn render(md: &str, theme: Theme, hl: &dyn Highlighter) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut bold = false;
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_buf = String::new();
    let mut list_stack: Vec<Option<u64>> = Vec::new();
    let mut list_item_index: Vec<u64> = Vec::new();

    for event in Parser::new(md) {
        match event {
            Event::Start(Tag::Strong) => bold = true,
            Event::End(TagEnd::Strong) => bold = false,

            Event::Start(Tag::Heading { level, .. }) => {
                flush_line(&mut current_spans, &mut lines);
                current_spans.push(Span::styled(heading_prefix(level), theme.accent()));
            }

            Event::End(TagEnd::Paragraph) => {
                flush_line(&mut current_spans, &mut lines);
                lines.push(Line::default());
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
                flush_line(&mut current_spans, &mut lines);
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

            Event::Code(text) => {
                current_spans.push(Span::styled(
                    format!(" {text} "),
                    theme.code_plain().bg(theme.code.bg),
                ));
            }

            Event::Text(text) => {
                if in_code_block {
                    code_buf.push_str(&text);
                } else {
                    let style = if bold {
                        theme.base().add_modifier(Modifier::BOLD)
                    } else {
                        theme.base()
                    };
                    for (i, segment) in text.split('\n').enumerate() {
                        if i > 0 {
                            flush_line(&mut current_spans, &mut lines);
                        }
                        if !segment.is_empty() {
                            current_spans.push(Span::styled(segment.to_owned(), style));
                        }
                    }
                }
            }

            Event::SoftBreak => current_spans.push(Span::raw(" ")),

            Event::End(TagEnd::Heading(_) | TagEnd::Item) | Event::HardBreak => {
                flush_line(&mut current_spans, &mut lines);
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

fn render_code_block(
    code: &str,
    lang: &str,
    theme: Theme,
    hl: &dyn Highlighter,
    lines: &mut Vec<Line<'static>>,
) {
    let highlighted = hl.highlight(lang, code, theme);
    for (i, hl_line) in highlighted.into_iter().enumerate() {
        let mut spans = vec![Span::styled("│ ", theme.muted())];
        spans.extend(
            hl_line
                .spans
                .into_iter()
                .map(|s| Span::styled(s.content.into_owned(), s.style.bg(theme.code.bg))),
        );
        if i == 0 && !lang.is_empty() {
            spans.push(Span::styled(format!("  {lang}"), theme.muted()));
        }
        lines.push(Line::from(spans));
    }
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
        Some(None) | None => "• ".to_owned(),
    };
    format!("{indent}{marker}")
}

fn flush_line(spans: &mut Vec<Span<'static>>, lines: &mut Vec<Line<'static>>) {
    if !spans.is_empty() {
        lines.push(Line::from(std::mem::take(spans)));
    }
}

fn heading_prefix(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 => "# ",
        HeadingLevel::H2 => "## ",
        HeadingLevel::H3 => "### ",
        _ => "#### ",
    }
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
    fn inline_code_uses_code_bg() {
        let theme = Theme::dark();
        let lines = render("use `foo` here", theme, &PlainHighlighter);
        let spans: Vec<_> = lines.iter().flat_map(|l| &l.spans).collect();
        assert!(spans.iter().any(|s| s.style.bg == Some(theme.code.bg)));
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
    fn heading_renders_prefix() {
        let lines = render_plain("## Section");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("##"));
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
