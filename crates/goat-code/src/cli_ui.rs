use std::io::IsTerminal;

use color_eyre::eyre::{Report, Result, eyre};
use dialoguer::{Input, Password, Select};
use goat_provider::AuthMethod;

use crate::{
    provider_table,
    style::{ColorMode, Palette},
    theme::goat_theme,
};

pub struct ProviderPick {
    pub id: String,
    pub status: String,
    pub status_palette: Palette,
}

pub enum AuthPick {
    OAuth,
    ApiKey,
}

pub fn terminal_required() -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return fail("interactive commands require a terminal");
    }
    Ok(())
}

pub fn success(text: &str) {
    println!("{}", ColorMode::detect().paint(text, Palette::Success));
}

pub fn warning(text: &str) {
    println!("{}", ColorMode::detect().paint(text, Palette::Warning));
}

pub fn oauth_status(text: &str) {
    let color = ColorMode::detect();
    if let Some((visit, code)) = parse_device_code_message(text) {
        println!(
            "  {} {}",
            color.paint("visit", Palette::Muted),
            color.paint(visit, Palette::Info)
        );
        println!(
            "  {} {}",
            color.paint("code", Palette::Muted),
            color.paint(code, Palette::Value)
        );
        return;
    }
    if let Some(url) = parse_browser_url(text) {
        println!("  {}", color.paint(url, Palette::Info));
        return;
    }
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        println!("  {}", color.paint(line, Palette::Info));
    }
}

fn parse_device_code_message(text: &str) -> Option<(String, String)> {
    let rest = text.strip_prefix("open ")?;
    let (url, code) = rest.split_once(" and enter code: ")?;
    Some((url.trim().to_owned(), code.trim().to_owned()))
}

fn parse_browser_url(text: &str) -> Option<String> {
    let (_, url) = text.split_once(":\n")?;
    let url = url.trim();
    (!url.is_empty()).then(|| url.to_owned())
}

pub fn pick_provider(items: &[ProviderPick]) -> Result<usize> {
    terminal_required()?;
    if items.is_empty() {
        return Err(report("no login-capable providers available"));
    }
    let color = ColorMode::detect_stderr();
    let labels: Vec<String> = items
        .iter()
        .map(|item| {
            provider_table::picker_label(color, &item.id, &item.status, item.status_palette)
        })
        .collect();
    let index = Select::with_theme(goat_theme())
        .with_prompt(select_prompt(color, "provider", None))
        .items(&labels)
        .default(0)
        .report(false)
        .interact_opt()
        .map_err(dialoguer_error)?
        .ok_or_else(|| report("provider login cancelled"))?;
    Ok(index)
}

pub fn pick_auth_method(provider: &str, method: AuthMethod) -> Result<AuthPick> {
    match method {
        AuthMethod::OAuth => Ok(AuthPick::OAuth),
        AuthMethod::ApiKey => Ok(AuthPick::ApiKey),
        AuthMethod::ApiKeyOrOAuth => {
            terminal_required()?;
            let color = ColorMode::detect_stderr();
            let labels = [
                provider_table::option_label(color, "device code", Palette::Value, "browser"),
                provider_table::option_label(color, "api key", Palette::Value, "secret key"),
            ];
            let index = Select::with_theme(goat_theme())
                .with_prompt(select_prompt(color, "method", Some(provider)))
                .items(&labels)
                .default(0)
                .report(false)
                .interact_opt()
                .map_err(dialoguer_error)?
                .ok_or_else(|| report("provider login cancelled"))?;
            Ok(if index == 0 {
                AuthPick::OAuth
            } else {
                AuthPick::ApiKey
            })
        }
        AuthMethod::None => Err(report(format!("{provider} requires no login"))),
    }
}

pub fn prompt_api_key(provider: &str) -> Result<String> {
    let color = ColorMode::detect_stderr();
    prompt_secret(&input_prompt(color, "api key", Some(provider)))
}

pub fn prompt_secret(prompt: &str) -> Result<String> {
    terminal_required()?;
    let secret = Password::with_theme(goat_theme())
        .with_prompt(prompt)
        .allow_empty_password(false)
        .report(false)
        .interact()
        .map_err(dialoguer_error)?;
    Ok(secret.trim().to_owned())
}

fn select_prompt(color: ColorMode, label: &str, provider: Option<&str>) -> String {
    match provider {
        Some(provider) => format!(
            "{} {}",
            color.paint(label, Palette::Muted),
            color.paint(provider, Palette::Provider)
        ),
        None => color.paint(label, Palette::Muted),
    }
}

fn input_prompt(color: ColorMode, label: &str, provider: Option<&str>) -> String {
    select_prompt(color, label, provider)
}

pub fn prompt_endpoint(default: Option<&str>) -> Result<String> {
    let color = ColorMode::detect_stderr();
    prompt_text(&input_prompt(color, "endpoint", None), default)
}

pub fn prompt_text(prompt: &str, default: Option<&str>) -> Result<String> {
    terminal_required()?;
    let mut input = Input::with_theme(goat_theme());
    input = input.with_prompt(prompt).report(false);
    if let Some(default) = default {
        input = input.default(default.to_owned());
    }
    let value = input.interact_text().map_err(dialoguer_error)?;
    Ok(value.trim().to_owned())
}

pub fn fail(message: impl Into<String>) -> Result<()> {
    Err(report(message))
}

pub fn fail_hint(message: impl Into<String>, hint: impl Into<String>) -> Result<()> {
    Err(report_hint(message, hint))
}

pub fn report(message: impl Into<String>) -> Report {
    eyre!(format_failure(&message.into(), None))
}

pub fn report_hint(message: impl Into<String>, hint: impl Into<String>) -> Report {
    eyre!(format_failure(&message.into(), Some(hint.into())))
}

fn dialoguer_error(err: impl std::fmt::Display) -> Report {
    report(format!("prompt failed: {err}"))
}

pub fn format_failure(message: &str, hint: Option<String>) -> String {
    let color = ColorMode::detect_stderr();
    let mut lines = vec![format!(
        "{} {}",
        color.paint("error:", Palette::Warning),
        color.paint(message, Palette::Value)
    )];
    if let Some(hint) = hint {
        lines.push(format!(
            "{} {}",
            color.paint("hint:", Palette::Muted),
            color.paint(hint, Palette::Muted)
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        format_failure, input_prompt, oauth_status, parse_browser_url, parse_device_code_message,
    };
    use crate::style::ColorMode;

    #[test]
    fn input_prompt_names_provider() {
        let prompt = input_prompt(ColorMode::Plain, "api key", Some("openrouter"));
        assert!(prompt.contains("api key"));
        assert!(prompt.contains("openrouter"));
    }

    #[test]
    fn parses_device_code_oauth_message() {
        let parsed = parse_device_code_message("open https://auth.example and enter code: ABCD");
        assert_eq!(
            parsed,
            Some(("https://auth.example".to_owned(), "ABCD".to_owned()))
        );
    }

    #[test]
    fn parses_browser_oauth_url() {
        let parsed = parse_browser_url(
            "opening browser to sign in… if it does not open, visit:\nhttps://example.com",
        );
        assert_eq!(parsed, Some("https://example.com".to_owned()));
    }

    #[test]
    fn failure_format_includes_hint() {
        let text = format_failure(
            "unknown provider",
            Some("run goat provider list".to_owned()),
        );
        assert!(text.contains("error:"));
        assert!(text.contains("unknown provider"));
        assert!(text.contains("hint:"));
        assert!(text.contains("run goat provider list"));
    }

    #[test]
    fn oauth_status_skips_blank_lines() {
        oauth_status("line one\n\nline two");
    }
}
