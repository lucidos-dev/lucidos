//! Coding picks must survive validation exactly; tolerant runtime defaults are
//! not permission to replace an explicit choice in saved owner content.

use super::SavedDraftSnapshot;
use crate::runtime::CodingAgent;

#[derive(Debug, PartialEq, Eq)]
pub struct ResolvedCodingSelection {
    pub backend: CodingAgent,
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl SavedDraftSnapshot {
    /// `bound_backend` belongs to an active thread. `default_backend` is the
    /// current composer preference, used only when no thread binding exists.
    /// Resolve that preference strictly before calling this method.
    pub fn resolve_coding_selection(
        &self,
        bound_backend: Option<CodingAgent>,
        default_backend: CodingAgent,
    ) -> Result<ResolvedCodingSelection, Box<dyn std::error::Error + Send + Sync>> {
        let pick = self.selection();
        if bound_backend.is_some_and(|bound| pick.coding_agent.is_some_and(|pick| pick != bound)) {
            return Err("draft_coding_backend_locked".into());
        }
        let backend = bound_backend
            .or(pick.coding_agent)
            .unwrap_or(default_backend);
        let model = crate::runtime::validate_coding_agent_model(backend, pick.cc_model.as_deref())
            .map_err(|e| format!("draft_coding_model_unsupported: {e}"))?;
        let effort = crate::runtime::validate_coding_agent_effort(
            backend,
            model.as_deref(),
            pick.cc_reasoning_effort.as_deref(),
        )
        .map_err(|e| format!("draft_coding_effort_unsupported: {e}"))?;
        if model != pick.cc_model || effort != pick.cc_reasoning_effort {
            return Err("draft_coding_settings_changed".into());
        }
        Ok(ResolvedCodingSelection {
            backend,
            model,
            effort,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot(selection: serde_json::Value) -> SavedDraftSnapshot {
        SavedDraftSnapshot::try_from((
            0,
            0,
            "saved".into(),
            json!([]),
            Some("claude_code".into()),
            Some(selection),
        ))
        .unwrap()
    }

    #[test]
    fn explicit_default_model_stays_explicit() {
        for backend in CodingAgent::ALL {
            let resolved = snapshot(json!({"ccModel":"default"}))
                .resolve_coding_selection(None, backend)
                .unwrap();
            assert_eq!(resolved.model.as_deref(), Some("default"));
            assert_eq!(resolved.effort, None);
        }
    }

    #[test]
    fn unset_coding_overrides_stay_unset_on_both_backends() {
        for backend in CodingAgent::ALL {
            for selection in [json!({}), json!({"ccModel":null,"ccReasoningEffort":null})] {
                assert_eq!(
                    snapshot(selection)
                        .resolve_coding_selection(None, backend)
                        .unwrap(),
                    ResolvedCodingSelection {
                        backend,
                        model: None,
                        effort: None
                    }
                );
            }
        }
    }

    #[test]
    fn active_backend_binding_wins_over_default_but_refuses_conflicting_pick() {
        let draft = snapshot(json!({}));
        assert_eq!(
            draft
                .resolve_coding_selection(Some(CodingAgent::Codex), CodingAgent::ClaudeCode)
                .unwrap()
                .backend,
            CodingAgent::Codex
        );
        assert!(snapshot(json!({"codingAgent":"claude-code"}))
            .resolve_coding_selection(Some(CodingAgent::Codex), CodingAgent::ClaudeCode)
            .is_err());
        assert_eq!(
            snapshot(json!({"codingAgent":"codex"}))
                .resolve_coding_selection(None, CodingAgent::ClaudeCode)
                .unwrap()
                .backend,
            CodingAgent::Codex
        );
    }

    #[test]
    fn explicit_coding_choices_are_never_trimmed_or_discarded() {
        for backend in CodingAgent::ALL {
            for selection in [
                json!({"ccModel":""}),
                json!({"ccModel":" default "}),
                json!({"ccModel":"not-a-model"}),
                json!({"ccReasoningEffort":""}),
                json!({"ccReasoningEffort":" high "}),
                json!({"ccReasoningEffort":"not-an-effort"}),
            ] {
                assert!(
                    snapshot(selection.clone())
                        .resolve_coding_selection(None, backend)
                        .is_err(),
                    "{backend:?}: {selection}"
                );
            }
        }
    }
}
