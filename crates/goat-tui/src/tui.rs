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

pub fn init(mouse_capture: bool) -> io::Result<DefaultTerminal> {
    let terminal = ratatui::init();
    if supports_keyboard_enhancement().unwrap_or(false) {
        execute!(
            io::stdout(),
            EnableBracketedPaste,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
        )?;
    } else {
        execute!(io::stdout(), EnableBracketedPaste)?;
    }
    if mouse_capture {
        execute!(io::stdout(), EnableMouseCapture)?;
    }
    Ok(terminal)
}

pub fn set_mouse_capture(enabled: bool) {
    if enabled {
        let _ = execute!(io::stdout(), EnableMouseCapture);
    } else {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
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
