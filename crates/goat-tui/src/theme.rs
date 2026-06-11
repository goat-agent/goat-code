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
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    bg: Color,
    fg: Color,
    user: Color,
    agent: Color,
    tool: Color,
    error: Color,
    muted: Color,
    accent: Color,
    border: Color,
    selection: Color,
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
            bg: Color::Rgb(0x12, 0x12, 0x14),
            fg: Color::Rgb(0xe6, 0xe6, 0xe6),
            user: Color::Rgb(0x7a, 0xa2, 0xf7),
            agent: Color::Rgb(0x9e, 0xce, 0x6a),
            tool: Color::Rgb(0xe0, 0xaf, 0x68),
            error: Color::Rgb(0xf7, 0x76, 0x8e),
            muted: Color::Rgb(0x6b, 0x70, 0x7c),
            accent: Color::Rgb(0xbb, 0x9a, 0xf7),
            border: Color::Rgb(0x2a, 0x2c, 0x32),
            selection: Color::Rgb(0x2f, 0x33, 0x3e),
            panel: Color::Rgb(0x1b, 0x1b, 0x1e),
            shell: Color::Rgb(0xdb, 0x4b, 0x4b),
            shell_dim: Color::Rgb(0x54, 0x29, 0x2e),
            code: CodePalette {
                bg: Color::Rgb(0x1a, 0x1b, 0x26),
                keyword: Color::Rgb(0xbb, 0x9a, 0xf7),
                string: Color::Rgb(0x9e, 0xce, 0x6a),
                comment: Color::Rgb(0x56, 0x5f, 0x89),
                number: Color::Rgb(0xff, 0x9e, 0x64),
                type_: Color::Rgb(0x2a, 0xc3, 0xde),
                function: Color::Rgb(0x7a, 0xa2, 0xf7),
            },
        }
    }

    pub const fn light() -> Self {
        Self {
            bg: Color::Rgb(0xfa, 0xfa, 0xfa),
            fg: Color::Rgb(0x1c, 0x1e, 0x22),
            user: Color::Rgb(0x2e, 0x5c, 0xc9),
            agent: Color::Rgb(0x2f, 0x7d, 0x32),
            tool: Color::Rgb(0xb5, 0x6a, 0x00),
            error: Color::Rgb(0xc6, 0x28, 0x28),
            muted: Color::Rgb(0x8a, 0x8f, 0x98),
            accent: Color::Rgb(0x6a, 0x3d, 0xc9),
            border: Color::Rgb(0xd9, 0xdc, 0xe1),
            selection: Color::Rgb(0xdd, 0xe3, 0xec),
            panel: Color::Rgb(0xee, 0xee, 0xf0),
            shell: Color::Rgb(0xb0, 0x35, 0x54),
            shell_dim: Color::Rgb(0xe6, 0xc2, 0xcb),
            code: CodePalette {
                bg: Color::Rgb(0xf0, 0xf0, 0xf5),
                keyword: Color::Rgb(0x6a, 0x3d, 0xc9),
                string: Color::Rgb(0x2f, 0x7d, 0x32),
                comment: Color::Rgb(0x9e, 0xa3, 0xb0),
                number: Color::Rgb(0xb5, 0x6a, 0x00),
                type_: Color::Rgb(0x00, 0x7a, 0x8a),
                function: Color::Rgb(0x2e, 0x5c, 0xc9),
            },
        }
    }

    pub fn base(self) -> Style {
        Style::new().fg(self.fg).bg(self.bg)
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
        Style::new().fg(self.panel)
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

    pub fn role_agent(self) -> Style {
        Style::new().fg(self.agent)
    }

    pub fn role_tool(self) -> Style {
        Style::new().fg(self.tool)
    }

    pub fn tool_name(self) -> Style {
        Style::new().fg(self.tool)
    }

    pub fn selected_row(self) -> Style {
        Style::new().bg(self.selection)
    }

    pub fn error(self) -> Style {
        Style::new().fg(self.error)
    }

    pub fn meter(self, pct: f32) -> Style {
        if pct >= METER_HIGH {
            self.error()
        } else if pct >= METER_WARN {
            self.tool_name()
        } else {
            self.muted()
        }
    }

    pub fn code_plain(self) -> Style {
        Style::new().fg(self.fg).bg(self.code.bg)
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
        match self.bg {
            Color::Rgb(r, g, b) => {
                let luminance = u32::from(r) + u32::from(g) + u32::from(b);
                luminance < 384
            }
            _ => true,
        }
    }
}
