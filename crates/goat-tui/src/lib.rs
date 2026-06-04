mod app;
mod composer;
pub mod highlight;
mod keymap;
pub mod markdown;
mod theme;
mod transcript;
mod tui;
mod view;

pub use app::run;
pub use theme::Theme;
pub use tui::install_hooks;
