use std::io;

use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::supports_keyboard_enhancement,
};
use ratatui::DefaultTerminal;

pub fn init() -> io::Result<DefaultTerminal> {
    let terminal = ratatui::init();
    if supports_keyboard_enhancement().unwrap_or(false) {
        execute!(
            io::stdout(),
            EnableBracketedPaste,
            EnableMouseCapture,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
        )?;
    } else {
        execute!(io::stdout(), EnableBracketedPaste, EnableMouseCapture)?;
    }
    Ok(terminal)
}

pub fn restore() {
    if supports_keyboard_enhancement().unwrap_or(false) {
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            DisableMouseCapture,
            PopKeyboardEnhancementFlags,
        );
    } else {
        let _ = execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture);
    }
    ratatui::restore();
}

pub fn install_hooks() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));
    Ok(())
}
