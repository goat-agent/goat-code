use std::io;

use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
};
use ratatui::DefaultTerminal;

pub fn init() -> io::Result<DefaultTerminal> {
    let terminal = ratatui::init();
    execute!(io::stdout(), EnableBracketedPaste)?;
    Ok(terminal)
}

pub fn restore() {
    let _ = execute!(io::stdout(), DisableBracketedPaste);
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
