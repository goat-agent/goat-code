#[derive(Debug, thiserror::Error)]
pub enum ComputerError {
    #[error("invalid action input: {0}")]
    InvalidInput(String),
    #[error("no monitor found")]
    NoMonitor,
    #[error("unknown key: {0}")]
    UnknownKey(String),
    #[error("screen capture failed: {0}")]
    Capture(#[from] xcap::XCapError),
    #[error("input connection failed: {0}")]
    Connection(#[from] enigo::NewConError),
    #[error("input simulation failed: {0}")]
    Input(#[from] enigo::InputError),
    #[error("image encode failed: {0}")]
    Encode(#[from] image::ImageError),
}
