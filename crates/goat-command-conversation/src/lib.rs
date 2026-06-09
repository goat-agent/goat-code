mod clear;
mod rename;
mod resume;

use goat_command::Command;

pub use clear::Clear;
pub use rename::Rename;
pub use resume::Resume;

pub fn all() -> Vec<Box<dyn Command>> {
    vec![Box::new(Clear), Box::new(Resume), Box::new(Rename)]
}
