use ratatui::style::{Color, Modifier, Style};

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
        }
    }

    pub fn base(self) -> Style {
        Style::new().fg(self.fg).bg(self.bg)
    }

    pub fn muted(self) -> Style {
        Style::new().fg(self.muted)
    }

    pub fn accent(self) -> Style {
        Style::new().fg(self.accent).add_modifier(Modifier::BOLD)
    }

    pub fn border(self) -> Style {
        Style::new().fg(self.border)
    }

    pub fn role_user(self) -> Style {
        Style::new().fg(self.user).add_modifier(Modifier::BOLD)
    }

    pub fn role_agent(self) -> Style {
        Style::new().fg(self.agent)
    }

    pub fn role_tool(self) -> Style {
        Style::new().fg(self.tool)
    }

    pub fn error(self) -> Style {
        Style::new().fg(self.error).add_modifier(Modifier::BOLD)
    }
}
