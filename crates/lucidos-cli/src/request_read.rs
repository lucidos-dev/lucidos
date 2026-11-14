//! `lucidos request-read yes|no`: the turn's *read decision* on this thread's
//! latest reply.
//!
//! The coding-agent counterpart of the chat agent's `request_read` LLM tool,
//! over `POST /api/v1/threads/<id>/read-request`, which records the same
//! events (ADR 0409, ADR 0417). `$LUCIDOS_THREAD_ID` names the thread and
//! there is no `--thread` flag, so a session can only decide for its own reply.

use clap::ValueEnum;

use crate::http::{client as http_client, send_and_print};
use crate::workspace::{BoxError, Workspace};

/// The answer: the reply is worth reading, or it is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum ReadAnswer {
    Yes,
    No,
}

impl ReadAnswer {
    fn body(self) -> serde_json::Value {
        serde_json::json!({ "read": self == Self::Yes })
    }
}

pub(crate) fn cmd_request_read(ws: &Workspace, answer: ReadAnswer) -> Result<(), BoxError> {
    let thread_id = std::env::var("LUCIDOS_THREAD_ID").map_err(|_| -> BoxError {
        "LUCIDOS_THREAD_ID is not set, so there is no thread whose reply to read. \
         This subcommand only works from inside a Lucidos coding-agent session."
            .into()
    })?;
    let url = format!(
        "{}/api/v1/threads/{}/read-request",
        ws.base_url(),
        thread_id
    );
    send_and_print("POST", &url, http_client()?.post(&url).json(&answer.body()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        answer: ReadAnswer,
    }

    #[test]
    fn each_answer_posts_its_boolean() {
        assert_eq!(ReadAnswer::Yes.body(), serde_json::json!({ "read": true }));
        assert_eq!(ReadAnswer::No.body(), serde_json::json!({ "read": false }));
    }

    #[test]
    fn the_bare_form_is_refused() {
        assert!(Cli::try_parse_from(["request-read"]).is_err());
        assert!(Cli::try_parse_from(["request-read", "maybe"]).is_err());
        assert_eq!(
            Cli::try_parse_from(["request-read", "no"]).unwrap().answer,
            ReadAnswer::No
        );
    }
}
