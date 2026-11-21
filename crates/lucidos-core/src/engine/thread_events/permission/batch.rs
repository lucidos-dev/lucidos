//! Exact owner-approved batch items and their receipts.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::legacy::{LegacyLane, LegacyPick};
use super::side_effect::SideEffectCategory;

/// The items a batch carries, and the digest the engine stored when it
/// worded them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Batch {
    pub items: Vec<BatchItem>,
    /// SHA-256 of the items, in hex. A batch whose items no longer hash to it
    /// runs nothing.
    pub digest: String,
}

pub fn items_digest(items: &[BatchItem]) -> Result<String, serde_json::Error> {
    use sha2::{Digest, Sha256};
    Ok(crate::core::hex::hex_lower(&Sha256::digest(
        serde_json::to_vec(items)?,
    )))
}

/// One exact item of a batch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BatchItem {
    /// Pick one option on an open card in a sub-thread.
    CardAnswer {
        thread_id: Uuid,
        tool_use_id: String,
        option_id: String,
        option_label: String,
        /// SHA-256 of the card's question and options when the batch was built.
        card_digest: String,
        /// The plan this pick approves, when the thread waits on one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan: Option<PlanBinding>,
    },
    /// Pick one button on an open tool-permission card in a sub-thread. The
    /// card's own resolver stores what the button stores.
    PermissionCardAnswer {
        thread_id: Uuid,
        lane: LegacyLane,
        request_id: String,
        pick: LegacyPick,
        option_label: String,
    },
    /// Widen one trigger's grants, as long as its definition is unchanged.
    TriggerGrant {
        trigger_id: String,
        /// The trigger's definition digest when the batch was built.
        definition_digest: String,
        /// The side-effect categories the grant adds.
        side_effects: Vec<SideEffectCategory>,
        /// Whether the grant lets the trigger's runs start coding agents.
        coding_agents: bool,
    },
    /// The engine's program grant card for one blocked trigger run. An intent
    /// run waits on it and resumes in `run_thread_id`. A script fire has
    /// ended, so its `queue_entry_id` names the fire a grant runs again.
    TriggerProgramGrant {
        trigger_id: String,
        definition_digest: String,
        pattern: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        queue_entry_id: Option<Uuid>,
        #[serde(alias = "failed_thread_id")]
        run_thread_id: Uuid,
    },
    /// The engine's folder grant card for one blocked trigger run, read as
    /// [`Self::TriggerProgramGrant`].
    TriggerFolderGrant {
        trigger_id: String,
        definition_digest: String,
        grant: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        queue_entry_id: Option<Uuid>,
        #[serde(alias = "failed_thread_id")]
        run_thread_id: Uuid,
    },
    /// Single-program review cards persisted before reviews were grouped.
    ReviewedTriggerProgramGrant {
        trigger_id: String,
        definition_digest: String,
        pattern: String,
    },
    /// An owner review of programs evidenced by one trigger's past runs.
    /// Nothing posts one now, but an open card from an older version still
    /// answers through it.
    ReviewedTriggerProgramGrants {
        trigger_id: String,
        definition_digest: String,
        patterns: Vec<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        selectable: bool,
    },
    /// Set the origin of a group of unclassified workspaces. Only entries
    /// still `unknown` move, and the gateway refuses any other.
    WorkspaceClassification {
        workspaces: Vec<String>,
        origin: ClassifiedOrigin,
        /// The agent's evidence beyond the names, quoted on the card.
        evidence: String,
    },
}

impl BatchItem {
    /// Validate a checkbox answer against the proposal covered by the digest.
    pub fn review_selection(
        &self,
        answer: &super::super::AnswerKind,
    ) -> Option<Result<Vec<String>, &'static str>> {
        use super::super::AnswerKind;
        let Self::ReviewedTriggerProgramGrants {
            patterns,
            selectable: true,
            ..
        } = self
        else {
            return None;
        };
        Some(match answer {
            AnswerKind::Canceled => Ok(vec![]),
            AnswerKind::MultiSelected {
                option_ids,
                text,
                image_hashes,
            } if text.is_none()
                && image_hashes.is_empty()
                && option_ids.iter().all(|id| patterns.contains(id)) =>
            {
                Ok(patterns
                    .iter()
                    .filter(|p| option_ids.contains(p))
                    .cloned()
                    .collect())
            }
            _ => Err("Choose the grants on the review card"),
        })
    }

    /// Read either review format while preserving the original item for its digest.
    pub fn reviewed_trigger_programs(&self) -> Option<(&str, &str, &[String])> {
        match self {
            Self::ReviewedTriggerProgramGrant {
                trigger_id,
                definition_digest,
                pattern,
            } => Some((trigger_id, definition_digest, std::slice::from_ref(pattern))),
            Self::ReviewedTriggerProgramGrants {
                trigger_id,
                definition_digest,
                patterns,
                ..
            } => Some((trigger_id, definition_digest, patterns)),
            _ => None,
        }
    }
}

/// An origin a classification may set. Never `unknown`: that is where every
/// entry starts. The gateway's registry is the authority on the full set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClassifiedOrigin {
    Agent,
    Demo,
    Eval,
    E2e,
    User,
}

impl ClassifiedOrigin {
    /// The wire name the gateway's classify route takes.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Demo => "demo",
            Self::Eval => "eval",
            Self::E2e => "e2e",
            Self::User => "user",
        }
    }
}

impl BatchItem {
    pub fn answers_a_card(&self) -> bool {
        matches!(
            self,
            Self::CardAnswer { .. } | Self::PermissionCardAnswer { .. }
        )
    }
}

/// What running one item did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BatchItemOutcome {
    Applied,
    /// Nothing ran for this item. `reason` names what changed.
    Skipped {
        reason: String,
    },
    /// Part of the item ran, then a refusal stopped it: some workspaces of a
    /// classification moved. `reason` says how many, and what stopped it.
    PartlyApplied {
        reason: String,
    },
}

/// The plan text a batch item bound its pick to: the committed plan at the
/// marker it saw.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanBinding {
    pub repo_root: String,
    pub branch_name: String,
    pub plan_path: String,
    /// When the marker was written. A re-mark moves it, so it needs a new yes.
    pub marked_at: chrono::DateTime<chrono::Utc>,
    /// SHA-256 of the committed plan text, in hex.
    pub digest: String,
}

#[cfg(test)]
mod review_selection_tests {
    use super::*;

    #[test]
    fn an_existing_grouped_review_keeps_its_original_digest_and_answer_mode() {
        let payload = r#"[{"kind":"reviewed_trigger_program_grants","trigger_id":"daily","definition_digest":"original","patterns":["Bash(curl:*)","Bash(git:*)"]}]"#;
        let items: Vec<BatchItem> = serde_json::from_str(payload).unwrap();
        assert_eq!(serde_json::to_string(&items).unwrap(), payload);
        let allow = super::super::super::AnswerKind::Selected {
            option_id: "opt-0".into(),
        };
        assert!(items[0].review_selection(&allow).is_none());
    }
}
