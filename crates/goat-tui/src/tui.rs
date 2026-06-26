use std::io;

use crossterm::{
    event::{
        DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::supports_keyboard_enhancement,
};
use ratatui::DefaultTerminal;
use ratatui_image::picker::Picker;

pub fn init(mouse_capture: bool) -> io::Result<(DefaultTerminal, Option<Picker>)> {
    let terminal = ratatui::init();
    let picker = crate::screenshot::query_picker();
    if supports_keyboard_enhancement().unwrap_or(false) {
        execute!(
            io::stdout(),
            EnableBracketedPaste,
            EnableFocusChange,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
        )?;
    } else {
        execute!(io::stdout(), EnableBracketedPaste, EnableFocusChange)?;
    }
    if mouse_capture {
        execute!(io::stdout(), EnableMouseCapture)?;
    }
    Ok((terminal, picker))
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
            DisableFocusChange,
            DisableMouseCapture,
            PopKeyboardEnhancementFlags,
        );
    } else {
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            DisableFocusChange,
            DisableMouseCapture
        );
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
