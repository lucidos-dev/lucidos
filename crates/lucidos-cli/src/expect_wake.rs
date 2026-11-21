//! `lucidos expect-wake <duration> --reason <text>`: when this thread expects
//! to wake next, from the waits and children it leaves pending.
//!
//! The coding-agent counterpart of the chat agent's `expect_wake` LLM tool,
//! over `POST /api/v1/threads/<id>/expected-wake`, which records the same
//! `WakeExpected` (ADR 0603). `$LUCIDOS_THREAD_ID` names the thread and there
//! is no `--thread` flag, so a session can only state its own wake.

use crate::http::{client as http_client, send_and_print};
use crate::workspace::{BoxError, Workspace};

/// Parse a span such as `90s`, `45m`, `2h` or `1h30m` into seconds. A bare
/// number is refused, so a unit is never guessed.
pub(crate) fn parse_duration_secs(text: &str) -> Result<u64, String> {
    let refuse = || {
        format!(
            "`{text}` is not a duration. Write a number and a unit: `90s`, `45m`, `2h` or \
             `1h30m`."
        )
    };
    let mut total = 0u64;
    let mut digits = String::new();
    for ch in text.trim().chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            continue;
        }
        let unit = match ch {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            _ => return Err(refuse()),
        };
        let n: u64 = digits.parse().map_err(|_| refuse())?;
        total = total.saturating_add(n.saturating_mul(unit));
        digits.clear();
    }
    if !digits.is_empty() || total == 0 {
        return Err(refuse());
    }
    Ok(total)
}

pub(crate) fn cmd_expect_wake(ws: &Workspace, in_secs: u64, reason: &str) -> Result<(), BoxError> {
    let thread_id = std::env::var("LUCIDOS_THREAD_ID").map_err(|_| -> BoxError {
        "LUCIDOS_THREAD_ID is not set, so there is no thread to state a wake for. \
         This subcommand only works from inside a Lucidos coding-agent session."
            .into()
    })?;
    let url = format!(
        "{}/api/v1/threads/{}/expected-wake",
        ws.base_url(),
        thread_id
    );
    let body = serde_json::json!({ "in_secs": in_secs, "reason": reason });
    send_and_print("POST", &url, http_client()?.post(&url).json(&body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duration_takes_units_and_sums_them() {
        assert_eq!(parse_duration_secs("90s"), Ok(90));
        assert_eq!(parse_duration_secs("45m"), Ok(2700));
        assert_eq!(parse_duration_secs("2h"), Ok(7200));
        assert_eq!(parse_duration_secs("1h30m"), Ok(5400));
    }

    #[test]
    fn a_duration_without_a_unit_is_refused() {
        for text in ["45", "", "m", "0m", "4x", "1.5h", "-5m"] {
            assert!(parse_duration_secs(text).is_err(), "{text}");
        }
    }
}
