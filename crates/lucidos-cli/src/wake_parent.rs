//! `lucidos wake-parent yes|no`: a child thread's *wake decision* on this
//! turn's report to its parent.
//!
//! The coding-agent counterpart of the chat agent's `wake_parent` LLM tool,
//! over `POST /api/v1/threads/<id>/wake-decision`, which records the same
//! events. `$LUCIDOS_THREAD_ID` names the thread and there is no `--thread`
//! flag, so a session can only decide for its own report.

use clap::ValueEnum;

use crate::http::{client as http_client, send_and_print};
use crate::workspace::{BoxError, Workspace};

/// The answer: the parent must act on this report now, or it need not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum WakeAnswer {
    Yes,
    No,
}

impl WakeAnswer {
    fn body(self) -> serde_json::Value {
        serde_json::json!({ "wake": self == Self::Yes })
    }
}

pub(crate) fn cmd_wake_parent(ws: &Workspace, answer: WakeAnswer) -> Result<(), BoxError> {
    let thread_id = std::env::var("LUCIDOS_THREAD_ID").map_err(|_| -> BoxError {
        "LUCIDOS_THREAD_ID is not set, so there is no thread whose report to decide on. \
         This subcommand only works from inside a Lucidos coding-agent session."
            .into()
    })?;
    let url = format!(
        "{}/api/v1/threads/{}/wake-decision",
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
        answer: WakeAnswer,
    }

    #[test]
    fn each_answer_posts_its_boolean() {
        assert_eq!(WakeAnswer::Yes.body(), serde_json::json!({ "wake": true }));
        assert_eq!(WakeAnswer::No.body(), serde_json::json!({ "wake": false }));
    }

    #[test]
    fn the_bare_form_is_refused() {
        assert!(Cli::try_parse_from(["wake-parent"]).is_err());
        assert!(Cli::try_parse_from(["wake-parent", "maybe"]).is_err());
        assert_eq!(
            Cli::try_parse_from(["wake-parent", "no"]).unwrap().answer,
            WakeAnswer::No
        );
    }
}
