mod clear;
mod resume;

use goat_command::Command;

pub use clear::Clear;
pub use resume::Resume;

pub fn all() -> Vec<Box<dyn Command>> {
    vec![Box::new(Clear), Box::new(Resume)]
}
