use std::time::{Duration, Instant};

use goat_protocol::Event;
use goat_provider::{Request, StreamError, now_secs};
use rand::RngExt;
use tokio_util::sync::CancellationToken;

use crate::{
    Ctx, LoopEnv, Run,
    rounds::{RoundEnd, RoundResult, run_round},
};

pub(crate) const MAX_ATTEMPTS: u32 = 6;
const BASE_DELAY: Duration = Duration::from_secs(1);
const MAX_DELAY: Duration = Duration::from_secs(30);
const MIN_RETRY_AFTER: Duration = Duration::from_secs(1);
const MAX_RETRY_AFTER: Duration = Duration::from_mins(10);
const RESET_SLICE: Duration = Duration::from_secs(30);
const MAX_RESET_WAIT: Duration = Duration::from_hours(8 * 24);

pub(crate) fn retryable(error: &StreamError) -> bool {
    matches!(
        error,
        StreamError::RateLimited { .. }
            | StreamError::Overloaded { .. }
            | StreamError::Transport { .. }
    )
}

pub(crate) fn reason_label(error: &StreamError) -> &'static str {
    match error {
        StreamError::RateLimited { .. } => "rate limited",
        StreamError::Overloaded { .. } => "overloaded",
        StreamError::Transport { .. } => "connection lost",
        StreamError::ContextOverflow { .. } => "context window exceeded",
        StreamError::Auth { .. } => "authentication failed",
        StreamError::InvalidRequest { .. } => "invalid request",
        StreamError::Other { .. } => "provider error",
    }
}

pub(crate) fn failure_message(error: &StreamError, target: &goat_protocol::ModelTarget) -> String {
    match error {
        StreamError::Auth { message } => format!(
            "authentication failed ({}/{}): {message}",
            target.provider, target.account,
        ),
        other => other.to_string(),
    }
}

pub(crate) fn error_hint(error: &StreamError) -> Option<String> {
    match error {
        StreamError::Auth { .. } => {
            Some("/config to re-login — progress saved, send a message to continue".to_owned())
        }
        StreamError::ContextOverflow { .. } => {
            Some("/compact to free context, then resend".to_owned())
        }
        StreamError::RateLimited { .. } | StreamError::Overloaded { .. } => {
            Some("wait a moment and resend, or /model to switch".to_owned())
        }
        StreamError::Transport { .. } => Some("check your connection and resend".to_owned()),
        StreamError::InvalidRequest { .. } | StreamError::Other { .. } => None,
    }
}

fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

pub(crate) fn backoff_delay(error: &StreamError, attempt: u32) -> Duration {
    if let StreamError::RateLimited {
        retry_after: Some(after),
        ..
    } = error
    {
        return (*after).clamp(MIN_RETRY_AFTER, MAX_RETRY_AFTER);
    }
    let exponential = BASE_DELAY.saturating_mul(2u32.saturating_pow(attempt.saturating_sub(1)));
    exponential
        .min(MAX_DELAY)
        .mul_f64(rand::rng().random_range(0.5..=1.0))
}

pub(crate) fn reset_at(error: &StreamError) -> Option<i64> {
    if let StreamError::RateLimited {
        resets_at,
        retry_after,
        ..
    } = error
    {
        if let Some(ts) = resets_at {
            return Some(*ts);
        }
        if let Some(after) = retry_after
            && *after > MAX_RETRY_AFTER
        {
            let secs = i64::try_from(after.as_secs()).unwrap_or(i64::MAX);
            return Some(now_secs().saturating_add(secs));
        }
    }
    None
}

pub(crate) fn reset_wait_target(error: &StreamError, last_reset: Option<i64>) -> Option<i64> {
    let ts = reset_at(error)?;
    let now = now_secs();
    if ts <= now {
        return None;
    }
    if last_reset.is_some_and(|prev| ts <= prev) {
        return None;
    }
    Some(ts.min(now + i64::try_from(MAX_RESET_WAIT.as_secs()).unwrap_or(i64::MAX)))
}

pub(crate) enum WaitOutcome {
    Retry,
    Cancelled,
}

pub(crate) async fn wait_until_reset(
    events: &tokio::sync::mpsc::Sender<Event>,
    id: goat_protocol::TaskId,
    reason: &str,
    target: i64,
    token: &CancellationToken,
) -> WaitOutcome {
    loop {
        let now = now_secs();
        let remaining = target.saturating_sub(now);
        if remaining <= 0 {
            return WaitOutcome::Retry;
        }
        let remaining_ms = u64::try_from(remaining)
            .unwrap_or(u64::MAX)
            .saturating_mul(1000);
        let _ = events
            .send(Event::Retrying {
                id,
                attempt: 1,
                max_attempts: 1,
                delay_ms: remaining_ms,
                reason: reason.to_owned(),
                resets_at: Some(target),
            })
            .await;
        let slice = RESET_SLICE.min(Duration::from_secs(
            u64::try_from(remaining).unwrap_or(u64::MAX),
        ));
        tokio::select! {
            biased;
            () = token.cancelled() => return WaitOutcome::Cancelled,
            () = tokio::time::sleep(slice) => {}
        }
    }
}

fn exhausted(mut result: RoundResult, attempts: u32, started: Instant) -> RoundResult {
    if let RoundEnd::Failed(
        StreamError::RateLimited { message, .. }
        | StreamError::Overloaded { message }
        | StreamError::Transport { message },
    ) = &mut result.end
    {
        *message = exhausted_message(message, attempts, started);
    }
    result
}

pub(crate) fn exhausted_message(message: &str, attempts: u32, started: Instant) -> String {
    format!(
        "gave up after {attempts} attempts ({}): {message}",
        format_elapsed(started.elapsed()),
    )
}

pub(crate) async fn run_round_with_retry(
    ctx: &Ctx<'_>,
    run: &Run<'_>,
    env: &LoopEnv<'_>,
    messages: &[goat_provider::Message],
    token: &CancellationToken,
) -> RoundResult {
    let started = Instant::now();
    let mut attempt = 1u32;
    let mut last_reset: Option<i64> = None;
    loop {
        let request = Request {
            model: env.target.model.clone(),
            messages: messages.to_vec(),
            tools: env.tool_defs.to_vec(),
            effort: env.target.effort,
            tool_choice: goat_provider::ToolChoice::Auto,
            temperature: None,
            max_tokens: None,
            system: None,
        };
        let result = run_round(ctx, run, env.provider, request, token).await;
        let RoundEnd::Failed(error) = &result.end else {
            return result;
        };
        if !retryable(error) {
            return result;
        }
        if let Some(target) = reset_wait_target(error, last_reset) {
            last_reset = Some(target);
            match wait_until_reset(ctx.events, run.id, reason_label(error), target, token).await {
                WaitOutcome::Retry => continue,
                WaitOutcome::Cancelled => return RoundResult::ended(RoundEnd::Cancelled),
            }
        }
        if attempt >= MAX_ATTEMPTS {
            return exhausted(result, attempt, started);
        }
        let delay = backoff_delay(error, attempt);
        let _ = ctx
            .events
            .send(Event::Retrying {
                id: run.id,
                attempt,
                max_attempts: MAX_ATTEMPTS,
                delay_ms: u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                reason: reason_label(error).to_owned(),
                resets_at: None,
            })
            .await;
        tokio::select! {
            biased;
            () = token.cancelled() => return RoundResult::ended(RoundEnd::Cancelled),
            () = tokio::time::sleep(delay) => {}
        }
        attempt += 1;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use goat_provider::{StreamError, now_secs};

    fn rate_limited_reset(resets_at: Option<i64>) -> StreamError {
        StreamError::rate_limited_at("limited", None, resets_at)
    }

    #[test]
    fn reset_target_accepts_future_reset() {
        let future = now_secs() + 3600;
        assert_eq!(
            super::reset_wait_target(&rate_limited_reset(Some(future)), None),
            Some(future)
        );
    }

    #[test]
    fn reset_target_rejects_past_reset() {
        let past = now_secs() - 3600;
        assert_eq!(
            super::reset_wait_target(&rate_limited_reset(Some(past)), None),
            None
        );
    }

    #[test]
    fn reset_target_rejects_absent_reset() {
        assert_eq!(
            super::reset_wait_target(&rate_limited_reset(None), None),
            None
        );
        assert_eq!(
            super::reset_wait_target(&StreamError::overloaded("busy"), None),
            None
        );
    }

    #[test]
    fn long_retry_after_promotes_to_precise_wait() {
        let long = super::MAX_RETRY_AFTER + Duration::from_mins(10);
        let error = StreamError::rate_limited("limited", Some(long));
        let target = super::reset_wait_target(&error, None).expect("promoted");
        let expected = now_secs() + i64::try_from(long.as_secs()).unwrap();
        assert!(
            (target - expected).abs() <= 2,
            "target {target} vs {expected}"
        );
    }

    #[test]
    fn short_retry_after_stays_in_backoff() {
        let short = super::MAX_RETRY_AFTER
            .checked_sub(Duration::from_secs(1))
            .unwrap();
        let error = StreamError::rate_limited("limited", Some(short));
        assert_eq!(super::reset_wait_target(&error, None), None);
    }

    #[test]
    fn reset_target_requires_forward_progress() {
        let now = now_secs();
        let earlier = now + 3600;
        let later = now + 7200;
        assert_eq!(
            super::reset_wait_target(&rate_limited_reset(Some(later)), Some(earlier)),
            Some(later)
        );
        assert_eq!(
            super::reset_wait_target(&rate_limited_reset(Some(earlier)), Some(later)),
            None
        );
        assert_eq!(
            super::reset_wait_target(&rate_limited_reset(Some(earlier)), Some(earlier)),
            None
        );
    }

    #[test]
    fn reset_target_caps_at_max_reset_wait() {
        let now = now_secs();
        let absurd = now + 400 * 24 * 60 * 60;
        let target = super::reset_wait_target(&rate_limited_reset(Some(absurd)), None).unwrap();
        let ceiling = now + i64::try_from(super::MAX_RESET_WAIT.as_secs()).unwrap();
        assert!(target <= ceiling);
        assert!(target < absurd);
    }

    #[tokio::test]
    async fn wait_until_reset_cancels_promptly() {
        let (events, _rx) = tokio::sync::mpsc::channel(16);
        let token = tokio_util::sync::CancellationToken::new();
        let target = now_secs() + 7 * 24 * 60 * 60;
        token.cancel();
        let outcome = super::wait_until_reset(
            &events,
            goat_protocol::TaskId(1),
            "rate limited",
            target,
            &token,
        )
        .await;
        assert!(matches!(outcome, super::WaitOutcome::Cancelled));
    }

    #[tokio::test]
    async fn wait_until_reset_retries_when_target_passed() {
        let (events, _rx) = tokio::sync::mpsc::channel(16);
        let token = tokio_util::sync::CancellationToken::new();
        let outcome = super::wait_until_reset(
            &events,
            goat_protocol::TaskId(1),
            "rate limited",
            now_secs() - 10,
            &token,
        )
        .await;
        assert!(matches!(outcome, super::WaitOutcome::Retry));
    }

    #[test]
    fn retry_after_overrides_backoff_curve() {
        let error = StreamError::rate_limited("slow down", Some(Duration::from_secs(45)));
        assert_eq!(super::backoff_delay(&error, 1), Duration::from_secs(45));
        let huge = StreamError::rate_limited("slow down", Some(Duration::from_secs(10_000)));
        assert_eq!(super::backoff_delay(&huge, 1), Duration::from_mins(10));
    }

    #[test]
    fn backoff_grows_exponentially_with_jitter() {
        let error = StreamError::overloaded("busy");
        for attempt in 1..=super::MAX_ATTEMPTS {
            let delay = super::backoff_delay(&error, attempt);
            let ceiling =
                Duration::from_secs(2u64.saturating_pow(attempt - 1)).min(Duration::from_secs(30));
            assert!(
                delay <= ceiling,
                "attempt {attempt}: {delay:?} > {ceiling:?}"
            );
            assert!(
                delay >= ceiling.mul_f64(0.5),
                "attempt {attempt}: {delay:?} below jitter floor"
            );
        }
    }

    #[test]
    fn fatal_errors_are_not_retryable() {
        assert!(!super::retryable(&StreamError::auth("nope")));
        assert!(!super::retryable(&StreamError::invalid_request("bad")));
        assert!(!super::retryable(&StreamError::context_overflow("full")));
        assert!(!super::retryable(&StreamError::other("quota")));
        assert!(super::retryable(&StreamError::transport("reset")));
    }

    #[test]
    fn auth_failure_message_carries_remediation() {
        let target = goat_protocol::ModelTarget {
            provider: "anthropic".into(),
            model: "m".into(),
            account: "work".into(),
            effort: None,
        };
        let message = super::failure_message(&StreamError::auth("expired"), &target);
        assert!(message.contains("anthropic/work"));
        let hint = super::error_hint(&StreamError::auth("expired")).unwrap();
        assert!(hint.contains("/config to re-login"));
        assert!(hint.contains("progress saved"));
    }

    #[test]
    fn error_hint_covers_retryable_and_overflow() {
        assert!(super::error_hint(&StreamError::rate_limited("x", None)).is_some());
        assert!(super::error_hint(&StreamError::transport("x")).is_some());
        assert!(
            super::error_hint(&StreamError::ContextOverflow {
                message: "x".into()
            })
            .is_some()
        );
        assert!(super::error_hint(&StreamError::other("x")).is_none());
    }
}
