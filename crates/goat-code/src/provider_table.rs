use crate::style::{ColorMode, Palette};

pub(crate) const PROVIDER_WIDTH: usize = 15;
pub(crate) const STATUS_WIDTH: usize = 10;
pub(crate) const PREFIX_WIDTH: usize = 1;

pub(crate) fn header(color: ColorMode, account: bool) -> String {
    let mut line = format!(
        "  {} {} {}",
        color.cell(" ", Palette::Muted, PREFIX_WIDTH),
        color.cell("provider", Palette::Muted, PROVIDER_WIDTH),
        color.cell("status", Palette::Muted, STATUS_WIDTH),
    );
    if account {
        line.push(' ');
        line.push_str(&color.paint("account", Palette::Muted));
    }
    line
}

pub(crate) fn row(
    color: ColorMode,
    prefix: &str,
    prefix_palette: Palette,
    id: &str,
    status: &str,
    status_palette: Palette,
    account: Option<&str>,
) -> String {
    let mut line = format!(
        "  {} {}",
        color.cell(prefix, prefix_palette, PREFIX_WIDTH),
        cells(color, id, status, status_palette),
    );
    if let Some(account) = account {
        line.push(' ');
        line.push_str(&color.paint(account, Palette::Value));
    }
    line
}

fn cells(color: ColorMode, id: &str, status: &str, status_palette: Palette) -> String {
    format!(
        "{} {}",
        color.cell(id, Palette::Provider, PROVIDER_WIDTH),
        color.cell(status, status_palette, STATUS_WIDTH)
    )
}

pub(crate) fn picker_label(
    color: ColorMode,
    id: &str,
    status: &str,
    status_palette: Palette,
) -> String {
    cells(color, id, status, status_palette)
}

pub(crate) const OPTION_WIDTH: usize = 14;

pub(crate) fn option_label(
    color: ColorMode,
    label: &str,
    label_palette: Palette,
    hint: &str,
) -> String {
    format!(
        "{} {}",
        color.cell(label, label_palette, OPTION_WIDTH),
        color.paint(hint, Palette::Muted)
    )
}

#[cfg(test)]
mod tests {
    use super::{OPTION_WIDTH, PROVIDER_WIDTH, STATUS_WIDTH, option_label, picker_label};
    use crate::style::{ColorMode, Palette};
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn picker_label_aligns_columns() {
        let label = picker_label(ColorMode::Plain, "openrouter", "missing", Palette::Warning);
        assert!(label.starts_with("openrouter"));
        assert!(label.contains("missing"));
        assert!(label.width() >= PROVIDER_WIDTH + 1 + STATUS_WIDTH);
    }

    #[test]
    fn option_label_aligns_columns() {
        let label = option_label(ColorMode::Plain, "device code", Palette::Value, "browser");
        assert!(label.starts_with("device code"));
        assert!(label.contains("browser"));
        assert!(label.width() >= OPTION_WIDTH + 1 + "browser".len());
    }
}
