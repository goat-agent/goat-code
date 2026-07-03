use ratatui::style::Color;

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
pub struct Palette {
    pub id: u8,
    pub bg: Color,
    pub fg: Color,
    pub dark: bool,
    pub user: Color,
    pub user_panel: Color,
    pub agent: Color,
    pub tool: Color,
    pub error: Color,
    pub muted: Color,
    pub accent: Color,
    pub success: Color,
    pub border: Color,
    pub border_dim: Color,
    pub panel: Color,
    pub shell: Color,
    pub shell_dim: Color,
    pub code: CodePalette,
}

impl Palette {
    pub const fn dark() -> Self {
        Self {
            id: 1,
            bg: Color::Rgb(0, 0, 0),
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
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    palette: Palette,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    pub const fn dark() -> Self {
        Self {
            palette: Palette::dark(),
        }
    }

    pub const fn light() -> Self {
        Self {
            palette: Palette::light(),
        }
    }

    pub fn base(self) -> ratatui::style::Style {
        let p = self.palette;
        ratatui::style::Style::new().fg(p.fg).bg(p.bg)
    }

    pub fn text(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.fg)
    }

    pub fn muted(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.muted)
    }

    pub fn key(self) -> ratatui::style::Style {
        ratatui::style::Style::new()
            .fg(self.palette.fg)
            .add_modifier(ratatui::style::Modifier::BOLD)
    }

    pub fn accent(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.accent)
    }

    pub fn border(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.border)
    }

    pub fn border_dim(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.border_dim)
    }

    pub fn shell(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.shell)
    }

    pub fn shell_dim(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.shell_dim)
    }

    pub fn role_user(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.user)
    }

    pub fn user_panel(self) -> ratatui::style::Style {
        ratatui::style::Style::new().bg(self.palette.user_panel)
    }

    pub fn role_agent(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.agent)
    }

    pub fn role_tool(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.tool)
    }

    pub fn tool_fn(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.accent)
    }

    pub fn tool_arg_value(self) -> ratatui::style::Style {
        self.muted()
    }

    pub fn error(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.error)
    }

    pub fn success(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.success)
    }

    pub fn error_body(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.error)
    }

    pub fn meter(self, pct: f32) -> ratatui::style::Style {
        if pct >= METER_HIGH {
            self.error()
        } else if pct >= METER_WARN {
            self.role_tool()
        } else {
            self.muted()
        }
    }

    pub fn code_plain(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.fg)
    }

    pub fn inline_code(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.accent)
    }

    pub fn fg_color(self) -> Color {
        self.palette.fg
    }

    pub fn accent_color(self) -> Color {
        self.palette.accent
    }

    pub fn hint_key(self) -> ratatui::style::Style {
        ratatui::style::Style::new().fg(self.palette.fg)
    }

    pub fn panel(self) -> ratatui::style::Style {
        let p = self.palette;
        ratatui::style::Style::new().fg(p.fg).bg(p.panel)
    }

    pub fn is_dark(self) -> bool {
        self.palette.dark
    }

    pub fn id(self) -> u8 {
        self.palette.id
    }

    pub fn code(self) -> CodePalette {
        self.palette.code
    }
}
