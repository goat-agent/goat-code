mod event;
mod op;
mod types;

pub use event::{AskOption, AskQuestion, Event, NotifyKind};
pub use op::Op;
pub use types::*;

#[cfg(test)]
mod tests {
    use super::{ToolImageData, ToolOutcome};

    #[test]
    fn tool_outcome_image_round_trips() {
        let outcome = ToolOutcome {
            ok: true,
            summary: Some("captured".to_owned()),
            image: Some(ToolImageData {
                media_type: "image/png".to_owned(),
                data: "AAAA".to_owned(),
            }),
        };
        let json = serde_json::to_string(&outcome).unwrap();
        let back: ToolOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(outcome, back);
    }

    #[test]
    fn tool_outcome_without_image_omits_field() {
        let outcome = ToolOutcome {
            ok: false,
            summary: None,
            image: None,
        };
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(!json.contains("image"));
        let back: ToolOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(outcome, back);
    }
}
