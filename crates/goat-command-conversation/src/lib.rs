mod clear;

use goat_command::Command;

pub use clear::Clear;

pub fn all() -> Vec<Box<dyn Command>> {
    vec![Box::new(Clear)]
}
