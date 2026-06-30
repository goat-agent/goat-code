use std::io::IsTerminal;

#[derive(Clone, Copy)]
pub enum ColorMode {
    Plain,
    Ansi,
}

impl ColorMode {
    pub fn detect() -> Self {
        if std::env::var_os("NO_COLOR").is_some() || !std::io::stdout().is_terminal() {
            Self::Plain
        } else {
            Self::Ansi
        }
    }

    pub fn paint(self, text: impl AsRef<str>, palette: Palette) -> String {
        let text = text.as_ref();
        match self {
            Self::Plain => text.to_owned(),
            Self::Ansi => format!("{}{}\x1b[0m", palette.code(), text),
        }
    }

    pub fn cell(self, text: impl AsRef<str>, palette: Palette, width: usize) -> String {
        let text = text.as_ref();
        let pad = width.saturating_sub(text.chars().count());
        format!("{}{}", self.paint(text, palette), " ".repeat(pad))
    }
}

#[derive(Clone, Copy)]
pub enum Palette {
    Info,
    Local,
    Muted,
    Provider,
    Success,
    Value,
    Warning,
}

impl Palette {
    fn code(self) -> &'static str {
        match self {
            Self::Info => "\x1b[38;5;75m",
            Self::Local => "\x1b[38;5;214m",
            Self::Muted => "\x1b[38;5;245m",
            Self::Provider => "\x1b[1;38;5;219m",
            Self::Success => "\x1b[38;5;84m",
            Self::Value => "\x1b[38;5;252m",
            Self::Warning => "\x1b[38;5;209m",
        }
    }
}

pub fn print_row(color: ColorMode, label: &str, value: impl AsRef<str>, palette: Palette) {
    println!(
        "  {} {}",
        color.cell(label, Palette::Muted, 10),
        color.paint(value, palette)
    );
}
