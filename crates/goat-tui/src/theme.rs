use ratatui::style::{Color, Modifier, Style};

use crate::layout::{METER_HIGH, METER_WARN};

#[derive(Debug, Clone, Copy)]
pub struct CodePalette {
    pub bg: Color,
    pub keyword: Color,
    pub string: Color,
    pub comment: Color,
    pub number: Color,
    pub type_: Color,
    pub function: Color,
    pub variable: Color,
    pub operator: Color,
    pub macro_: Color,
    pub property: Color,
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    id: u8,
    bg: Color,
    fg: Color,
    dark: bool,
    user: Color,
    user_panel: Color,
    agent: Color,
    tool: Color,
    error: Color,
    muted: Color,
    accent: Color,
    success: Color,
    border: Color,
    border_dim: Color,
    panel: Color,
    shell: Color,
    shell_dim: Color,
    pub code: CodePalette,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    pub const fn dark() -> Self {
        Self {
            id: 1,
            bg: Color::Reset,
            dark: true,
            fg: Color::Rgb(0xd7, 0xda, 0xe0),
            user: Color::Rgb(0x7d, 0x9b, 0xd4),
            user_panel: Color::Rgb(0x20, 0x20, 0x20),
            agent: Color::Rgb(0x6f, 0xb3, 0xa8),
            tool: Color::Rgb(0xcf, 0x9b, 0x6b),
            error: Color::Rgb(0xc9, 0x7a, 0x7a),
            muted: Color::Rgb(0x7a, 0x80, 0x8c),
            accent: Color::Rgb(0xa9, 0x8f, 0xd0),
            success: Color::Rgb(0x8f, 0xb9, 0x8a),
            border: Color::Rgb(0x2a, 0x2c, 0x32),
            border_dim: Color::Rgb(0x22, 0x24, 0x29),
            panel: Color::Rgb(0x1b, 0x1b, 0x1e),
            shell: Color::Rgb(0xdb, 0x4b, 0x4b),
            shell_dim: Color::Rgb(0x54, 0x29, 0x2e),
            code: CodePalette {
                bg: Color::Reset,
                keyword: Color::Rgb(0x56, 0x9c, 0xd6),
                string: Color::Rgb(0xce, 0x91, 0x78),
                comment: Color::Rgb(0x6a, 0x99, 0x55),
                number: Color::Rgb(0xb5, 0xce, 0xa8),
                type_: Color::Rgb(0x4e, 0xc9, 0xb0),
                function: Color::Rgb(0xdc, 0xdc, 0xaa),
                variable: Color::Rgb(0x9c, 0xda, 0xfe),
                operator: Color::Rgb(0xd4, 0xd4, 0xd4),
                macro_: Color::Rgb(0xc5, 0x86, 0xc0),
                property: Color::Rgb(0x9c, 0xda, 0xfe),
            },
        }
    }

    pub const fn light() -> Self {
        Self {
            id: 2,
            bg: Color::Rgb(0xfa, 0xfa, 0xfa),
            dark: false,
            fg: Color::Rgb(0x1c, 0x1e, 0x22),
            user: Color::Rgb(0x2e, 0x5c, 0xc9),
            user_panel: Color::Rgb(0xf1, 0xf1, 0xf1),
            agent: Color::Rgb(0x2f, 0x7d, 0x32),
            tool: Color::Rgb(0xb5, 0x6a, 0x00),
            error: Color::Rgb(0xc6, 0x28, 0x28),
            muted: Color::Rgb(0x8a, 0x8f, 0x98),
            accent: Color::Rgb(0x6a, 0x3d, 0xc9),
            success: Color::Rgb(0x2f, 0x7d, 0x32),
            border: Color::Rgb(0xd9, 0xdc, 0xe1),
            border_dim: Color::Rgb(0xe6, 0xe8, 0xec),
            panel: Color::Rgb(0xee, 0xee, 0xf0),
            shell: Color::Rgb(0xb0, 0x35, 0x54),
            shell_dim: Color::Rgb(0xe6, 0xc2, 0xcb),
            code: CodePalette {
                bg: Color::Reset,
                keyword: Color::Rgb(0x00, 0x00, 0xff),
                string: Color::Rgb(0xa3, 0x15, 0x15),
                comment: Color::Rgb(0x00, 0x80, 0x00),
                number: Color::Rgb(0x09, 0x88, 0x58),
                type_: Color::Rgb(0x26, 0x7f, 0x99),
                function: Color::Rgb(0x79, 0x5e, 0x26),
                variable: Color::Rgb(0x00, 0x16, 0x80),
                operator: Color::Rgb(0x3b, 0x3b, 0x3b),
                macro_: Color::Rgb(0xaf, 0x00, 0xdb),
                property: Color::Rgb(0x00, 0x16, 0x80),
            },
        }
    }

    pub fn base(self) -> Style {
        Style::new().fg(self.fg).bg(self.bg)
    }

    pub fn text(self) -> Style {
        Style::new().fg(self.fg)
    }

    pub fn muted(self) -> Style {
        Style::new().fg(self.muted)
    }

    pub fn key(self) -> Style {
        Style::new().fg(self.fg).add_modifier(Modifier::BOLD)
    }

    pub fn accent(self) -> Style {
        Style::new().fg(self.accent)
    }

    pub fn border(self) -> Style {
        Style::new().fg(self.border)
    }

    pub fn border_dim(self) -> Style {
        Style::new().fg(self.border_dim)
    }

    pub fn shell(self) -> Style {
        Style::new().fg(self.shell)
    }

    pub fn shell_dim(self) -> Style {
        Style::new().fg(self.shell_dim)
    }

    pub fn role_user(self) -> Style {
        Style::new().fg(self.user)
    }

    pub fn user_panel(self) -> Style {
        Style::new().bg(self.user_panel)
    }

    pub fn role_agent(self) -> Style {
        Style::new().fg(self.agent)
    }

    pub fn role_tool(self) -> Style {
        Style::new().fg(self.tool)
    }

    pub fn error(self) -> Style {
        Style::new().fg(self.error)
    }

    pub fn success(self) -> Style {
        Style::new().fg(self.success)
    }

    pub fn error_body(self) -> Style {
        Style::new().fg(self.error)
    }

    pub fn meter(self, pct: f32) -> Style {
        if pct >= METER_HIGH {
            self.error()
        } else if pct >= METER_WARN {
            self.role_tool()
        } else {
            self.muted()
        }
    }

    pub fn code_plain(self) -> Style {
        Style::new().fg(self.fg)
    }

    pub fn inline_code(self) -> Style {
        Style::new().fg(self.accent)
    }

    pub fn fg_color(self) -> Color {
        self.fg
    }

    pub fn accent_color(self) -> Color {
        self.accent
    }

    pub fn hint_key(self) -> Style {
        Style::new().fg(self.fg)
    }

    pub fn panel(self) -> Style {
        Style::new().fg(self.fg).bg(self.panel)
    }

    pub fn is_dark(self) -> bool {
        self.dark
    }

    pub fn id(self) -> u8 {
        self.id
    }
}
