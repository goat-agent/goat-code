use goat_protocol::{AuthMethod, LoginCredential, LoginProvider};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::theme::Theme;

pub enum LoginOutcome {
    Pending,
    Submit {
        provider: String,
        credential: LoginCredential,
    },
}

enum Stage {
    Select,
    Key,
    Waiting,
}

pub struct Login {
    providers: Vec<LoginProvider>,
    cursor: usize,
    secret: String,
    stage: Stage,
    status: Option<String>,
}

impl Login {
    pub fn new(providers: Vec<LoginProvider>) -> Self {
        Self {
            providers,
            cursor: 0,
            secret: String::new(),
            stage: Stage::Select,
            status: None,
        }
    }

    pub fn move_up(&mut self) {
        if matches!(self.stage, Stage::Select) {
            self.cursor = self.cursor.saturating_sub(1);
        }
    }

    pub fn move_down(&mut self) {
        if matches!(self.stage, Stage::Select) && self.cursor + 1 < self.providers.len() {
            self.cursor += 1;
        }
    }

    pub fn on_char(&mut self, ch: char) {
        if matches!(self.stage, Stage::Key) {
            self.secret.push(ch);
        }
    }

    pub fn insert_str(&mut self, text: &str) {
        if matches!(self.stage, Stage::Key) {
            self.secret.push_str(text);
        }
    }

    pub fn backspace(&mut self) {
        if matches!(self.stage, Stage::Key) {
            self.secret.pop();
        }
    }

    pub fn set_status(&mut self, message: String) {
        self.status = Some(message);
    }

    pub fn enter(&mut self) -> LoginOutcome {
        let Some(provider) = self.providers.get(self.cursor) else {
            return LoginOutcome::Pending;
        };
        match self.stage {
            Stage::Select => match provider.method {
                AuthMethod::ApiKey => {
                    self.stage = Stage::Key;
                    LoginOutcome::Pending
                }
                AuthMethod::OAuth => {
                    let provider = provider.id.clone();
                    self.stage = Stage::Waiting;
                    self.status = Some("opening browser…".to_owned());
                    LoginOutcome::Submit {
                        provider,
                        credential: LoginCredential::OAuth,
                    }
                }
                AuthMethod::None => {
                    self.status = Some("no login required".to_owned());
                    LoginOutcome::Pending
                }
            },
            Stage::Key => LoginOutcome::Submit {
                provider: provider.id.clone(),
                credential: LoginCredential::ApiKey(std::mem::take(&mut self.secret)),
            },
            Stage::Waiting => LoginOutcome::Pending,
        }
    }

    pub fn desired_height(&self) -> u16 {
        let rows = match self.stage {
            Stage::Select => self.providers.len().max(1),
            Stage::Key | Stage::Waiting => 2,
        };
        u16::try_from(rows)
            .unwrap_or(u16::MAX)
            .min(10)
            .saturating_add(3)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        frame.render_widget(Clear, area);
        let block = Block::new()
            .borders(Borders::ALL)
            .border_style(theme.border())
            .style(theme.base());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let rows = usize::from(inner.height.saturating_sub(1));
        let mut lines: Vec<Line> = Vec::new();
        let mut cursor: Option<u16> = None;

        match self.stage {
            Stage::Select => {
                lines.push(Line::from(Span::styled(
                    " login — choose a provider",
                    theme.accent(),
                )));
                for (index, provider) in self.providers.iter().take(rows).enumerate() {
                    let style = if index == self.cursor {
                        theme.selected()
                    } else {
                        theme.base()
                    };
                    let method = match provider.method {
                        AuthMethod::ApiKey => "api key",
                        AuthMethod::OAuth => "browser",
                        AuthMethod::None => "no auth",
                    };
                    lines.push(Line::from(Span::styled(
                        format!("  {}   ({method}) ", provider.id),
                        style,
                    )));
                }
            }
            Stage::Key => {
                let id = self
                    .providers
                    .get(self.cursor)
                    .map_or("", |p| p.id.as_str());
                lines.push(Line::from(Span::styled(
                    format!(" login: {id}"),
                    theme.accent(),
                )));
                let masked = "\u{2022}".repeat(self.secret.chars().count());
                let prefix = " key> ";
                lines.push(Line::from(Span::styled(
                    format!("{prefix}{masked}"),
                    theme.base(),
                )));
                let col = prefix.chars().count() + masked.chars().count();
                cursor = Some(u16::try_from(col).unwrap_or(u16::MAX));
            }
            Stage::Waiting => {
                let id = self
                    .providers
                    .get(self.cursor)
                    .map_or("", |p| p.id.as_str());
                lines.push(Line::from(Span::styled(
                    format!(" login: {id}"),
                    theme.accent(),
                )));
            }
        }
        if let Some(status) = &self.status {
            lines.push(Line::from(Span::styled(
                format!("  {status}"),
                theme.muted(),
            )));
        }
        frame.render_widget(Paragraph::new(lines), inner);

        if let Some(col) = cursor {
            let x = inner.x + col;
            frame.set_cursor_position((x.min(inner.right().saturating_sub(1)), inner.y + 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use goat_protocol::{AuthMethod, LoginCredential, LoginProvider};

    use super::{Login, LoginOutcome};

    fn providers() -> Vec<LoginProvider> {
        vec![
            LoginProvider {
                id: "openai".to_owned(),
                method: AuthMethod::ApiKey,
            },
            LoginProvider {
                id: "openai-codex".to_owned(),
                method: AuthMethod::OAuth,
            },
        ]
    }

    #[test]
    fn api_key_provider_prompts_then_submits() {
        let mut login = Login::new(providers());
        assert!(matches!(login.enter(), LoginOutcome::Pending));
        for ch in "sk-1".chars() {
            login.on_char(ch);
        }
        match login.enter() {
            LoginOutcome::Submit {
                provider,
                credential,
            } => {
                assert_eq!(provider, "openai");
                assert!(matches!(credential, LoginCredential::ApiKey(key) if key == "sk-1"));
            }
            LoginOutcome::Pending => panic!("expected submit"),
        }
    }

    #[test]
    fn oauth_provider_submits_without_key() {
        let mut login = Login::new(providers());
        login.move_down();
        match login.enter() {
            LoginOutcome::Submit {
                provider,
                credential,
            } => {
                assert_eq!(provider, "openai-codex");
                assert!(matches!(credential, LoginCredential::OAuth));
            }
            LoginOutcome::Pending => panic!("expected oauth submit"),
        }
    }
}
