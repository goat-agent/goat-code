mod app;
mod command;
mod composer;
mod config;
pub mod highlight;
mod keymap;
pub mod markdown;
mod overlay;
mod picker;
pub mod symbols;
mod theme;
mod toast;
mod transcript;
mod tui;
mod view;

pub use app::run;
pub use theme::Theme;
pub use tui::install_hooks;
