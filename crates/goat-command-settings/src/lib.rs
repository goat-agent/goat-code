mod config;
mod effort;
mod model;
mod plan;
mod usage;

use goat_command::Command;

pub use config::Config;
pub use effort::Effort;
pub use model::Model;
pub use plan::Plan;
pub use usage::Usage;

pub fn all() -> Vec<Box<dyn Command>> {
    vec![
        Box::new(Model),
        Box::new(Effort),
        Box::new(Config),
        Box::new(Usage),
        Box::new(Plan),
    ]
}
