use std::time::Duration;

use goat_provider::StreamError;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: Option<ErrorBody>,
}

#[derive(Default, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    code: Option<u16>,
    #[serde(default)]
    message: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    details: Vec<Value>,
}

fn parse_body(body: &str) -> ErrorBody {
    serde_json::from_str::<ErrorEnvelope>(body)
        .ok()
        .and_then(|envelope| envelope.error)
        .unwrap_or_default()
}

fn retry_delay(details: &[Value]) -> Option<Duration> {
    let raw = details.iter().find_map(|detail| {
        detail
            .get("@type")
            .and_then(Value::as_str)
            .filter(|kind| kind.ends_with("google.rpc.RetryInfo"))?;
        detail.get("retryDelay").and_then(Value::as_str)
    })?;
    let seconds: f64 = raw.trim_end_matches('s').parse().ok()?;
    Some(Duration::from_secs_f64(seconds.max(0.0)))
}

fn overflow_message(message: &str) -> bool {
    message.contains("token count") && message.contains("exceeds")
}

pub(crate) fn classify_http(status: reqwest::StatusCode, body: &str) -> StreamError {
    let parsed = parse_body(body);
    let message = if parsed.message.is_empty() {
        format!("{status}: {body}")
    } else {
        parsed.message
    };
    let code = parsed.code.unwrap_or(status.as_u16());
    match (code, parsed.status.as_str()) {
        (429, _) | (_, "RESOURCE_EXHAUSTED") => {
            StreamError::rate_limited(message, retry_delay(&parsed.details))
        }
        (401 | 403, _) | (_, "UNAUTHENTICATED" | "PERMISSION_DENIED") => StreamError::auth(message),
        (400, _) | (_, "INVALID_ARGUMENT") => {
            if overflow_message(&message) {
                StreamError::context_overflow(message)
            } else {
                StreamError::invalid_request(message)
            }
        }
        (code, _) if (500..600).contains(&code) => StreamError::overloaded(message),
        (_, "UNAVAILABLE" | "INTERNAL" | "DEADLINE_EXCEEDED") => StreamError::overloaded(message),
        (code, _) if (400..500).contains(&code) => StreamError::invalid_request(message),
        _ => StreamError::other(message),
    }
}

#[cfg(test)]
mod tests {
    use goat_provider::StreamError;

    fn http(status: u16, body: &str) -> StreamError {
        super::classify_http(reqwest::StatusCode::from_u16(status).unwrap(), body)
    }

    #[test]
    fn resource_exhausted_with_retry_info() {
        let error = http(
            429,
            r#"{"error":{"code":429,"message":"Quota exceeded","status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"58s"}]}}"#,
        );
        assert_eq!(
            error,
            StreamError::rate_limited("Quota exceeded", Some(std::time::Duration::from_secs(58)))
        );
    }

    #[test]
    fn unauthenticated_is_auth() {
        let error = http(
            401,
            r#"{"error":{"code":401,"message":"Request had invalid authentication credentials","status":"UNAUTHENTICATED"}}"#,
        );
        assert!(matches!(error, StreamError::Auth { .. }));
    }

    #[test]
    fn token_overflow_is_context_overflow() {
        let error = http(
            400,
            r#"{"error":{"code":400,"message":"The input token count (1300000) exceeds the maximum number of tokens allowed (1048576).","status":"INVALID_ARGUMENT"}}"#,
        );
        assert!(matches!(error, StreamError::ContextOverflow { .. }));
    }

    #[test]
    fn unavailable_is_overloaded() {
        let error = http(
            503,
            r#"{"error":{"code":503,"message":"The service is currently unavailable.","status":"UNAVAILABLE"}}"#,
        );
        assert!(matches!(error, StreamError::Overloaded { .. }));
    }

    #[test]
    fn plain_400_is_invalid_request() {
        let error = http(
            400,
            r#"{"error":{"code":400,"message":"Invalid JSON payload","status":"INVALID_ARGUMENT"}}"#,
        );
        assert!(matches!(error, StreamError::InvalidRequest { .. }));
    }
}
