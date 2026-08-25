//! Command-guard LLM *judge* (ADR 0002, Phase 3) — the fallible half of the
//! hybrid classifier.
//!
//! The static [`static_classify`](crate::engine::command_guard::static_classify)
//! fast-path settles the catastrophic deny-list and an obviously-safe allowlist;
//! everything else is the *ambiguous middle* and lands here. A cheap, fast model
//! (Haiku by default, configurable via `model_command_judge`) reads the one
//! command and returns a [`JudgeVerdict`] — a [`RiskLane`] plus a one-line
//! summary for the permission card. It errs toward *ask*: anything it can't
//! confidently call safe becomes `IrreversibleDanger`.
//!
//! Infra failures (no provider, a failed/empty call) surface as `Err` so the
//! caller can fall back to the static "dangerous" list
//! ([`fallback_classify`](crate::engine::command_guard::fallback_classify))
//! rather than punish the user for an LLM hiccup. A *parseable-but-unclear*
//! response is NOT
//! an error — it resolves to `IrreversibleDanger` (ask), per the design's
//! tie-break.

use serde_json::Value;

use crate::engine::command_guard::{JudgeInput, RiskLane, SideEffectCategory};
use crate::engine::LucidosEngine;
use crate::llm::provider::{LlmProvider, Message, MessageContent};
use crate::llm::tool_names as tn;

/// The classifier rubric the judge runs under. Seeded with the safe/dangerous
/// examples from the design doc so a small model stays calibrated.
const JUDGE_SYSTEM_PROMPT: &str = r#"You are a security classifier for a personal AI assistant that runs shell and Python commands on the user's machine. The assistant works inside a single workspace directory (its current directory); reads anywhere on the machine and writes under the workspace are normal and expected.

Classify exactly ONE command into one risk lane and return STRICT JSON.

Lanes:
- "safe": reads, data crunching, builds/tests, downloads (HTTP GET), in-workspace writes, reading or editing files ANYWHERE on the machine — anything with no irreversible real-world side-effect and no destruction outside the workspace.
- "reversible": destruction confined to the workspace (e.g. rm or overwrite of files under the workspace) — recoverable from version control.
- "irreversible": (1) an irreversible real-world side-effect, OR (2) destruction of content OUTSIDE the workspace.
    Examples of (1): sending email or messages (mail, sendmail, osascript driving Mail/Messages), a mutating HTTP request (POST/PUT/DELETE/PATCH, a data upload), a cloud-service mutation (gh/aws/gcloud create/delete/push), Python requests.post / httpx / smtplib, publishing a package, anything that spends money or changes external state.
    Examples of (2): deleting or overwriting files outside the workspace (rm/mv/dd on system paths, the home directory, another repo).

IMPORTANT: reading or editing files OUTSIDE the workspace is a WANTED feature, not a threat — classify out-of-workspace READS and EDITS as "safe" (or "reversible" if it overwrites a file). Only out-of-workspace DESTRUCTION is "irreversible".

Tie-breaks: when unsure between "safe" and a danger lane, pick the danger lane. When unsure between "reversible" and "irreversible", pick "irreversible".

When (and only when) the lane is "irreversible", also classify the side-effect into one "category" — this gates whether an unattended scheduled trigger is allowed to run it:
- "email": sending email or messages (mail, sendmail, osascript driving Mail/Messages, Python smtplib).
- "external_api": a mutating outbound HTTP request (POST/PUT/DELETE/PATCH or data upload; Python requests/httpx writes).
- "cloud_cli": a cloud-service mutation via gh, aws, or gcloud.
- "out_of_workspace_destruction": deleting or overwriting files outside the workspace.
- "other": any irreversible side-effect that fits none of the above.
Pick the single best-fitting category; use "other" if unsure. For "safe" and "reversible" lanes, set "category" to null.

Return ONLY this JSON object, no prose and no code fences:
{"lane":"safe|reversible|irreversible","category":"email|external_api|cloud_cli|out_of_workspace_destruction|other|null","summary":"one short sentence the user sees on an approval card, e.g. 'Sends an email via the mail command.'","reason":"brief justification"}"#;

/// The judge's classification of one ambiguous command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgeVerdict {
    pub lane: RiskLane,
    /// The side-effect category — `Some` only when `lane == IrreversibleDanger`
    /// (gates the trigger side-effect grant); `None` for safe/reversible.
    pub category: Option<SideEffectCategory>,
    /// One-line card text shown to the user when the lane is `IrreversibleDanger`.
    pub summary: String,
    /// The judge's brief justification — logged, not shown to the user.
    pub reason: String,
}

impl JudgeVerdict {
    /// The verdict used when the model responded but produced nothing we can
    /// read as a lane — err toward *ask* (the design's tie-break). An
    /// unclassifiable irreversible command is [`SideEffectCategory::Other`], so
    /// an unattended trigger blocks it unless it granted `other`.
    fn uncertain() -> Self {
        Self {
            lane: RiskLane::IrreversibleDanger,
            category: Some(SideEffectCategory::Other),
            summary: "Could not classify this command; treating it as a possible irreversible side-effect.".to_string(),
            reason: "judge response was not parseable".to_string(),
        }
    }
}

/// Build the per-command user message: the tool kind, the out-of-workspace risk
/// signal, and the command text itself.
fn build_judge_user_prompt(input: &JudgeInput) -> String {
    let kind = match input.tool_name.as_str() {
        tn::RUN_PYTHON | tn::RUN_PYTHON_BACKGROUND => "Python code",
        _ => "Shell command",
    };
    let escapes = if input.out_of_workspace {
        "Static analysis flagged a filesystem target OUTSIDE the workspace.\n"
    } else {
        ""
    };
    format!(
        "Tool: {kind}\n{escapes}Classify this {kind_lower}:\n```\n{cmd}\n```",
        kind = kind,
        escapes = escapes,
        kind_lower = kind.to_ascii_lowercase(),
        cmd = input.command,
    )
}

/// Parse the judge's raw response into a [`JudgeVerdict`]. Robust to code fences
/// and surrounding prose; an unreadable response resolves to
/// [`JudgeVerdict::uncertain`] (ask) rather than failing.
pub fn parse_judge_response(raw: &str) -> JudgeVerdict {
    let cleaned = strip_code_fences(raw);
    let value = serde_json::from_str::<Value>(&cleaned).ok().or_else(|| {
        extract_json_object(&cleaned).and_then(|s| serde_json::from_str::<Value>(&s).ok())
    });
    let Some(value) = value else {
        return JudgeVerdict::uncertain();
    };
    let lane = value
        .get("lane")
        .and_then(Value::as_str)
        .map(parse_lane)
        .unwrap_or(RiskLane::IrreversibleDanger);
    // The category only matters for the irreversible lane (it gates the trigger
    // side-effect grant). An irreversible verdict with a missing/unrecognized
    // category falls back to `Other` so a trigger must explicitly grant it.
    let category = (lane == RiskLane::IrreversibleDanger).then(|| {
        value
            .get("category")
            .and_then(Value::as_str)
            .map(parse_category)
            .unwrap_or(SideEffectCategory::Other)
    });
    let summary = value
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| default_summary(lane).to_string());
    let reason = value
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    JudgeVerdict {
        lane,
        category,
        summary,
        reason,
    }
}

/// Map the judge's `category` string to a [`SideEffectCategory`], erring toward
/// [`SideEffectCategory::Other`] on any unrecognized value. Only consulted for
/// the irreversible lane.
fn parse_category(s: &str) -> SideEffectCategory {
    match s.trim().to_ascii_lowercase().as_str() {
        "email" => SideEffectCategory::Email,
        "external_api" | "externalapi" | "http" => SideEffectCategory::ExternalApi,
        "cloud_cli" | "cloudcli" | "cloud" => SideEffectCategory::CloudCli,
        "out_of_workspace_destruction" | "outofworkspacedestruction" | "destruction" => {
            SideEffectCategory::OutOfWorkspaceDestruction
        }
        _ => SideEffectCategory::Other,
    }
}

/// Map the judge's `lane` string to a [`RiskLane`], erring toward ask on any
/// unrecognized value. Never returns `Catastrophic` — that lane is the static
/// pass's exclusive, deterministic responsibility.
fn parse_lane(s: &str) -> RiskLane {
    match s.trim().to_ascii_lowercase().as_str() {
        "safe" => RiskLane::Safe,
        "reversible" | "reversible_danger" | "reversibledanger" => RiskLane::ReversibleDanger,
        _ => RiskLane::IrreversibleDanger,
    }
}

/// Fallback card text when the judge gave a lane but no usable summary. Only the
/// `IrreversibleDanger` text is ever shown (the other lanes don't prompt).
fn default_summary(lane: RiskLane) -> &'static str {
    match lane {
        RiskLane::Safe => "Runs a command with no irreversible side-effect.",
        RiskLane::ReversibleDanger => {
            "Deletes or overwrites files inside the workspace (recoverable)."
        }
        RiskLane::Catastrophic | RiskLane::IrreversibleDanger => {
            "May cause an irreversible real-world side-effect."
        }
    }
}

/// Strip a leading ```/```json fence and trailing ``` (Flash/Haiku sometimes
/// wrap JSON in a code block despite the instruction not to).
fn strip_code_fences(text: &str) -> String {
    let trimmed = text.trim();
    let without_opening = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);
    without_opening
        .trim()
        .strip_suffix("```")
        .unwrap_or(without_opening.trim())
        .trim()
        .to_string()
}

/// Extract the first `{...}` object from a string that has prose around the
/// JSON. Returns `None` if there's no balanced-looking object.
fn extract_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| text[start..=end].to_string())
}

/// Run one judge classification against an arbitrary [`LlmProvider`] — the
/// provider-agnostic core of [`LucidosEngine::judge_command`], split out so it
/// can be exercised with a stubbed provider offline.
///
/// Returns `Err` on infra failure (the call errors or the response is empty); a
/// non-empty but unclear response is `Ok(JudgeVerdict::uncertain())` (ask).
pub(crate) async fn judge_with_provider<P: LlmProvider + ?Sized>(
    provider: &P,
    input: &JudgeInput,
    // The `reasoning_command_judge` preference, the other half of the judge's
    // *model selection*.
    reasoning_effort: &str,
) -> Result<JudgeVerdict, Box<dyn std::error::Error + Send + Sync>> {
    let messages = vec![Message {
        role: "user".to_string(),
        content: MessageContent::Text(build_judge_user_prompt(input)),
    }];
    let response = provider
        .chat(
            messages,
            vec![],
            None,
            Some(JUDGE_SYSTEM_PROMPT),
            None,
            Some(reasoning_effort),
        )
        .await?;
    let raw = response.content.unwrap_or_default();
    if raw.trim().is_empty() {
        return Err("command judge returned an empty response".into());
    }
    Ok(parse_judge_response(&raw))
}

impl LucidosEngine {
    /// Ask the configured judge model to classify one ambiguous command.
    ///
    /// Returns `Err` only on infra failure — no LLM provider is configured, the
    /// provider can't be built for `model`, the call errors, or the response is
    /// empty — so the caller falls back to the static "dangerous" list. A
    /// non-empty but unclear response is `Ok(JudgeVerdict::uncertain())` (ask).
    pub(crate) async fn judge_command(
        &self,
        model: &str,
        input: &JudgeInput,
    ) -> Result<JudgeVerdict, Box<dyn std::error::Error + Send + Sync>> {
        let Some(extractor) = self.extractor.as_ref() else {
            return Err("command judge unavailable: no LLM provider configured".into());
        };
        let budget = crate::engine::aux_purpose::UNCAPTURED_CALL_BUDGET;
        let provider = extractor.provider_for_model(model, budget.attempt_timeout)?;
        let effort = crate::core::PreferenceStore::command_judge_reasoning(&self.pool).await;
        // Under the budget's whole-call deadline. A user waits on the
        // permission card this verdict fills in, and the caller's fallback to
        // the static list beats an open-ended wait.
        tokio::time::timeout(
            budget.deadline,
            judge_with_provider(provider.as_ref(), input, &effort),
        )
        .await
        .unwrap_or_else(|_| {
            Err(format!("command judge timed out after {:?}", budget.deadline).into())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::{LlmResponse, ToolDefinition};
    use crate::llm::TokenCallback;

    fn ji(tool: &str, cmd: &str, oow: bool) -> JudgeInput {
        JudgeInput {
            tool_name: tool.to_string(),
            command: cmd.to_string(),
            out_of_workspace: oow,
            fast_path_refused: false,
        }
    }

    /// A stubbed [`LlmProvider`] that echoes a fixed response — lets us exercise
    /// the judge call → parse → verdict path deterministically and offline (the
    /// real-model path is the same code through [`LucidosEngine::judge_command`]).
    struct StubProvider {
        response: Result<String, String>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for StubProvider {
        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model_override: Option<&str>,
            _system_prompt: Option<&str>,
            _on_token: Option<TokenCallback>,
            _reasoning_effort: Option<&str>,
        ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
            match &self.response {
                Ok(content) => Ok(serde_json::from_value(serde_json::json!({
                    "content": content,
                    "tool_calls": [],
                }))
                .unwrap()),
                Err(e) => Err(e.clone().into()),
            }
        }
    }

    fn stub(content: &str) -> StubProvider {
        StubProvider {
            response: Ok(content.to_string()),
        }
    }

    #[tokio::test]
    async fn stubbed_judge_classifies_each_lane() {
        // Mutating POST → ask, tagged external_api.
        let v = judge_with_provider(
            &stub(r#"{"lane":"irreversible","category":"external_api","summary":"Posts data to an API.","reason":"POST"}"#),
            &ji(tn::RUN_BASH, "curl -X POST https://api/charge", false),
            "none",
        )
        .await
        .unwrap();
        assert_eq!(v.lane, RiskLane::IrreversibleDanger);
        assert_eq!(v.category, Some(SideEffectCategory::ExternalApi));
        assert_eq!(v.summary, "Posts data to an API.");

        // GET → safe, no category.
        let v = judge_with_provider(
            &stub(r#"{"lane":"safe","summary":"Reads a URL.","reason":"GET"}"#),
            &ji(tn::RUN_BASH, "curl https://api/data", false),
            "none",
        )
        .await
        .unwrap();
        assert_eq!(v.lane, RiskLane::Safe);
        assert_eq!(v.category, None, "non-irreversible lanes carry no category");

        // Irreversible with a missing/unknown category → Other (trigger must
        // explicitly grant `other`).
        let v = judge_with_provider(
            &stub(
                r#"{"lane":"irreversible","summary":"Does something irreversible.","reason":"?"}"#,
            ),
            &ji(tn::RUN_BASH, "weird-tool --send", false),
            "none",
        )
        .await
        .unwrap();
        assert_eq!(v.category, Some(SideEffectCategory::Other));

        // In-workspace rm → reversible.
        let v = judge_with_provider(
            &stub(r#"{"lane":"reversible","summary":"Deletes workspace files.","reason":"in-ws"}"#),
            &ji(tn::RUN_BASH, "rm -rf data/tmp", false),
            "none",
        )
        .await
        .unwrap();
        assert_eq!(v.lane, RiskLane::ReversibleDanger);
    }

    #[tokio::test]
    async fn stubbed_judge_empty_response_is_infra_error() {
        // Empty content → Err, so the caller falls back to the static list.
        let v = judge_with_provider(&stub("   "), &ji(tn::RUN_BASH, "x", false), "none").await;
        assert!(v.is_err(), "empty response must be an infra error");
    }

    #[tokio::test]
    async fn stubbed_judge_call_error_propagates() {
        let provider = StubProvider {
            response: Err("network down".to_string()),
        };
        let v = judge_with_provider(&provider, &ji(tn::RUN_BASH, "x", false), "none").await;
        assert!(v.is_err(), "provider error must propagate as Err");
    }

    #[test]
    fn parses_each_lane() {
        assert_eq!(
            parse_judge_response(r#"{"lane":"safe","summary":"reads a file","reason":"read"}"#)
                .lane,
            RiskLane::Safe
        );
        assert_eq!(
            parse_judge_response(
                r#"{"lane":"reversible","summary":"deletes data/x","reason":"in-ws"}"#
            )
            .lane,
            RiskLane::ReversibleDanger
        );
        assert_eq!(
            parse_judge_response(
                r#"{"lane":"irreversible","summary":"sends mail","reason":"email"}"#
            )
            .lane,
            RiskLane::IrreversibleDanger
        );
    }

    #[test]
    fn lane_aliases_and_case() {
        for s in [
            "IRREVERSIBLE",
            " Irreversible_Danger ",
            "irreversibledanger",
        ] {
            assert_eq!(parse_lane(s), RiskLane::IrreversibleDanger, "{s}");
        }
        for s in ["reversible_danger", "ReversibleDanger"] {
            assert_eq!(parse_lane(s), RiskLane::ReversibleDanger, "{s}");
        }
        assert_eq!(parse_lane("SAFE"), RiskLane::Safe);
    }

    #[test]
    fn parses_category_for_irreversible_only() {
        // Category is read on the irreversible lane …
        let v = parse_judge_response(
            r#"{"lane":"irreversible","category":"email","summary":"sends mail","reason":"x"}"#,
        );
        assert_eq!(v.category, Some(SideEffectCategory::Email));
        // … and ignored on safe/reversible (None regardless of the field).
        let v = parse_judge_response(
            r#"{"lane":"safe","category":"email","summary":"reads","reason":"x"}"#,
        );
        assert_eq!(v.category, None);
    }

    #[test]
    fn category_aliases_and_unknown_fall_back_to_other() {
        for s in ["external_api", "externalapi", "HTTP"] {
            assert_eq!(parse_category(s), SideEffectCategory::ExternalApi, "{s}");
        }
        assert_eq!(parse_category("cloud"), SideEffectCategory::CloudCli);
        assert_eq!(
            parse_category("out_of_workspace_destruction"),
            SideEffectCategory::OutOfWorkspaceDestruction
        );
        assert_eq!(parse_category("nonsense"), SideEffectCategory::Other);
    }

    #[test]
    fn unknown_lane_errs_toward_ask() {
        assert_eq!(parse_lane("maybe"), RiskLane::IrreversibleDanger);
        assert_eq!(parse_lane(""), RiskLane::IrreversibleDanger);
        // A whole object missing the lane key → ask.
        assert_eq!(
            parse_judge_response(r#"{"summary":"x","reason":"y"}"#).lane,
            RiskLane::IrreversibleDanger
        );
    }

    #[test]
    fn tolerates_code_fences_and_prose() {
        let fenced = "```json\n{\"lane\":\"safe\",\"summary\":\"ok\",\"reason\":\"r\"}\n```";
        assert_eq!(parse_judge_response(fenced).lane, RiskLane::Safe);

        let prose = "Here is my verdict:\n{\"lane\":\"irreversible\",\"summary\":\"posts data\",\"reason\":\"POST\"}\nThat's all.";
        let v = parse_judge_response(prose);
        assert_eq!(v.lane, RiskLane::IrreversibleDanger);
        assert_eq!(v.summary, "posts data");
    }

    #[test]
    fn unparseable_response_is_uncertain_ask() {
        let v = parse_judge_response("the model rambled with no json at all");
        assert_eq!(v.lane, RiskLane::IrreversibleDanger);
        assert!(!v.summary.is_empty());
    }

    #[test]
    fn blank_summary_falls_back_to_default() {
        let v = parse_judge_response(r#"{"lane":"irreversible","summary":"   ","reason":"r"}"#);
        assert_eq!(v.lane, RiskLane::IrreversibleDanger);
        assert!(v.summary.contains("irreversible"));
    }

    #[test]
    fn user_prompt_labels_tool_and_marks_escape() {
        let p = build_judge_user_prompt(&ji(tn::RUN_BASH, "rm -rf /etc/x", true));
        assert!(p.contains("Shell command"));
        assert!(p.contains("OUTSIDE the workspace"));
        assert!(p.contains("rm -rf /etc/x"));

        let p = build_judge_user_prompt(&ji(tn::RUN_PYTHON, "requests.post(u)", false));
        assert!(p.contains("Python code"));
        assert!(!p.contains("OUTSIDE the workspace"));
    }
}
