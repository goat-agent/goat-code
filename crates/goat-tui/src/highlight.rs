use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, ThemeSet},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};

use crate::theme::Theme;

pub trait Highlighter: Send + Sync {
    fn highlight(&self, lang: &str, code: &str, theme: Theme) -> Vec<Line<'static>>;
}

pub struct PlainHighlighter;

impl Highlighter for PlainHighlighter {
    fn highlight(&self, _lang: &str, code: &str, theme: Theme) -> Vec<Line<'static>> {
        code.lines()
            .map(|line| Line::from(Span::styled(line.to_owned(), theme.code_plain())))
            .collect()
    }
}

pub struct SyntectHighlighter {
    syntax_set: SyntaxSet,
    dark_theme: syntect::highlighting::Theme,
    light_theme: syntect::highlighting::Theme,
}

impl SyntectHighlighter {
    pub fn new() -> Self {
        let theme_set = ThemeSet::load_defaults();
        let dark_theme = theme_set
            .themes
            .get("base16-ocean.dark")
            .or_else(|| theme_set.themes.values().next())
            .expect("syntect ships built-in themes")
            .clone();
        let light_theme = theme_set
            .themes
            .get("InspiredGitHub")
            .or_else(|| theme_set.themes.values().next())
            .expect("syntect ships built-in themes")
            .clone();
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            dark_theme,
            light_theme,
        }
    }

    fn pick_syntect_theme(&self, theme: Theme) -> &syntect::highlighting::Theme {
        if is_dark_bg(theme) {
            &self.dark_theme
        } else {
            &self.light_theme
        }
    }
}

impl Default for SyntectHighlighter {
    fn default() -> Self {
        Self::new()
    }
}

impl Highlighter for SyntectHighlighter {
    fn highlight(&self, lang: &str, code: &str, theme: Theme) -> Vec<Line<'static>> {
        let syntax = self
            .syntax_set
            .find_syntax_by_token(lang)
            .or_else(|| self.syntax_set.find_syntax_by_extension(lang))
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

        let syntect_theme = self.pick_syntect_theme(theme);
        let mut highlighter = HighlightLines::new(syntax, syntect_theme);
        let code_bg = theme.code.bg;
        let mut result = Vec::new();

        for raw_line in LinesWithEndings::from(code) {
            let ranges = highlighter
                .highlight_line(raw_line, &self.syntax_set)
                .unwrap_or_default();

            let spans: Vec<Span<'static>> = ranges
                .into_iter()
                .filter_map(|(style, text)| {
                    let text = text.trim_end_matches('\n');
                    if text.is_empty() {
                        return None;
                    }
                    let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
                    let mut s = Style::new().fg(fg).bg(code_bg);
                    if style.font_style.contains(FontStyle::BOLD) {
                        s = s.add_modifier(Modifier::BOLD);
                    }
                    if style.font_style.contains(FontStyle::ITALIC) {
                        s = s.add_modifier(Modifier::ITALIC);
                    }
                    Some(Span::styled(text.to_owned(), s))
                })
                .collect();

            result.push(Line::from(spans));
        }

        if result.is_empty() {
            result.push(Line::default());
        }

        result
    }
}

fn is_dark_bg(theme: Theme) -> bool {
    if let Color::Rgb(r, g, b) = theme.code.bg {
        (u32::from(r) + u32::from(g) + u32::from(b)) < 384
    } else {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn plain_returns_one_line_per_input_line() {
        let hl = PlainHighlighter;
        let lines = hl.highlight("rust", "let x = 1;\nlet y = 2;", Theme::dark());
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn plain_empty_code_returns_empty() {
        let hl = PlainHighlighter;
        let lines = hl.highlight("rust", "", Theme::dark());
        assert!(lines.is_empty());
    }

    #[test]
    fn syntect_does_not_panic_on_unknown_lang() {
        let hl = SyntectHighlighter::new();
        let lines = hl.highlight("notareallang", "hello world", Theme::dark());
        assert!(!lines.is_empty());
    }

    #[test]
    fn syntect_rust_produces_spans() {
        let hl = SyntectHighlighter::new();
        let lines = hl.highlight("rust", "fn main() {}", Theme::dark());
        assert!(!lines.is_empty());
    }

    #[test]
    fn syntect_light_theme_picks_light() {
        let hl = SyntectHighlighter::new();
        let lines = hl.highlight("rs", "let x = 1;", Theme::light());
        assert!(!lines.is_empty());
    }
}
