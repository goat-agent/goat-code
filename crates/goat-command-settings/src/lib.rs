mod config;
mod model;

use goat_command::Command;

pub use config::Config;
pub use model::Model;

pub fn all() -> Vec<Box<dyn Command>> {
    vec![Box::new(Model), Box::new(Config)]
}
