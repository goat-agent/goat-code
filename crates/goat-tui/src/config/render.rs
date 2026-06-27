use goat_protocol::{AccountEntry, AuthMethod};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    overlay::{clamp_u16, hint_line, selection_row},
    symbols,
    theme::Theme,
};

use super::state::{FIELD_LABEL_W, Field};

pub(super) fn appearance_row(
    theme: Theme,
    width: usize,
    selected: bool,
    label: &str,
    first_active: bool,
    first: &str,
    second: &str,
) -> Line<'static> {
    let label_style = if selected { theme.key() } else { theme.base() };
    let first_dot = if first_active {
        symbols::ui::DOT_FULL
    } else {
        symbols::ui::DOT_EMPTY
    };
    let second_dot = if first_active {
        symbols::ui::DOT_EMPTY
    } else {
        symbols::ui::DOT_FULL
    };
    let first_style = if first_active {
        theme.accent()
    } else {
        theme.muted()
    };
    let second_style = if first_active {
        theme.muted()
    } else {
        theme.accent()
    };
    selection_row(
        theme,
        selected,
        width,
        vec![
            Span::styled(format!("{label:<12}"), label_style),
            Span::styled(format!("{first_dot} {first:<6}"), first_style),
            Span::styled(format!("{second_dot} {second}"), second_style),
        ],
        None,
    )
}

pub(super) fn method_label(method: AuthMethod) -> &'static str {
    match method {
        AuthMethod::ApiKey => "api key",
        AuthMethod::OAuth => "browser",
        AuthMethod::ApiKeyOrOAuth => "api key / browser",
        AuthMethod::None => "no auth",
    }
}

pub(super) fn provider_method(entry: &AccountEntry) -> AuthMethod {
    entry.login
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_adding(
    frame: &mut Frame,
    area: Rect,
    theme: Theme,
    provider: &str,
    method: AuthMethod,
    name: &str,
    key: &str,
    field: Field,
    error: Option<&str>,
) {
    let [title_area, _, name_area, key_area, _, error_area, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);

    let title = Line::from(vec![
        Span::styled(format!(" {provider}"), theme.key()),
        Span::styled(
            format!("{}new account", symbols::ui::SEPARATOR),
            theme.muted(),
        ),
    ]);
    frame.render_widget(Paragraph::new(title), title_area);

    let api_key = !matches!(method, AuthMethod::OAuth);
    let value_cols = usize::from(area.width).saturating_sub(3 + FIELD_LABEL_W + 1);
    let name_active = field == Field::Name;
    let name_label_style = if name_active {
        theme.accent()
    } else {
        theme.muted()
    };
    let shown_name = input_tail(name, value_cols);
    let name_cols = shown_name.width();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("   {:<FIELD_LABEL_W$}", "name"), name_label_style),
            Span::styled(shown_name, theme.base()),
        ])),
        name_area,
    );
    if name_active {
        place_cursor(frame, name_area, name_cols);
    }

    if api_key {
        let key_active = field == Field::Key;
        let key_label_style = if key_active {
            theme.accent()
        } else {
            theme.muted()
        };
        let mask_cols = key.chars().count().min(value_cols);
        let masked = symbols::ui::MASK.repeat(mask_cols);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("   {:<FIELD_LABEL_W$}", "key"), key_label_style),
                Span::styled(masked, theme.base()),
            ])),
            key_area,
        );
        if key_active {
            place_cursor(frame, key_area, mask_cols);
        }
    }

    if let Some(message) = error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("   {} {message}", symbols::ui::CROSS),
                theme.error(),
            )))
            .wrap(Wrap { trim: false }),
            error_area,
        );
    }

    if api_key {
        frame.render_widget(
            Paragraph::new(hint_line(
                &[
                    (symbols::key::ENTER, "save"),
                    (symbols::key::TAB, "next field"),
                    (symbols::key::ESC, "cancel"),
                ],
                theme,
            )),
            hint_area,
        );
    } else {
        frame.render_widget(
            Paragraph::new(hint_line(
                &[
                    (symbols::key::ENTER, "open browser"),
                    (symbols::key::ESC, "cancel"),
                ],
                theme,
            )),
            hint_area,
        );
    }
}

fn input_tail(value: &str, max_cols: usize) -> String {
    if value.width() <= max_cols {
        return value.to_owned();
    }
    let mut cols = 0usize;
    let mut tail: Vec<char> = Vec::new();
    for c in value.chars().rev() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if cols + w > max_cols {
            break;
        }
        cols += w;
        tail.push(c);
    }
    tail.iter().rev().collect()
}

fn place_cursor(frame: &mut Frame, area: Rect, value_cols: usize) {
    let prefix = clamp_u16(3 + FIELD_LABEL_W);
    let col = clamp_u16(value_cols).saturating_add(prefix);
    let x = area.x + col.min(area.width.saturating_sub(1));
    frame.set_cursor_position((x, area.y));
}

pub(super) fn render_waiting(
    frame: &mut Frame,
    area: Rect,
    theme: Theme,
    provider: &str,
    name: &str,
    status: Option<&str>,
) {
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!(" {provider}"), theme.key()),
            Span::styled(symbols::ui::SEPARATOR, theme.muted()),
            Span::styled(name.to_owned(), theme.base()),
        ]),
        Line::from(Span::raw("")),
    ];
    if let Some(msg) = status {
        lines.push(Line::from(Span::styled(format!("   {msg}"), theme.muted())));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

pub(super) fn render_choosing(
    frame: &mut Frame,
    area: Rect,
    theme: Theme,
    provider: &str,
    method: AuthMethod,
) {
    let [title_area, _, api_area, browser_area, _, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);

    let title = Line::from(vec![
        Span::styled(format!(" {provider}"), theme.key()),
        Span::styled(
            format!("{}sign in with", symbols::ui::SEPARATOR),
            theme.muted(),
        ),
    ]);
    frame.render_widget(Paragraph::new(title), title_area);

    let width = usize::from(area.width);
    let row = |selected: bool, label: &str| {
        let style = if selected { theme.key() } else { theme.base() };
        selection_row(
            theme,
            selected,
            width,
            vec![Span::styled(label.to_owned(), style)],
            None,
        )
    };
    frame.render_widget(
        Paragraph::new(row(matches!(method, AuthMethod::ApiKey), "api key")),
        api_area,
    );
    frame.render_widget(
        Paragraph::new(row(matches!(method, AuthMethod::OAuth), "browser")),
        browser_area,
    );
    frame.render_widget(
        Paragraph::new(hint_line(
            &[
                (symbols::key::ARROWS_UPDOWN, "choose"),
                (symbols::key::ENTER, "continue"),
                (symbols::key::ESC, "cancel"),
            ],
            theme,
        )),
        hint_area,
    );
}
