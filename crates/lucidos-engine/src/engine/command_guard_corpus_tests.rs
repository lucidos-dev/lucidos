use super::{action_for_lane, EventChannel, GuardAction};
use crate::engine::command_guard::{RiskLane, SideEffectCategory, StaticVerdict};
use crate::engine::command_guard_corpus::GuardCorpus;
use crate::engine::command_judge_questions::{self, CATEGORY, LANE};
use crate::llm::judgment::{Answer, Answers, ChoiceAnswer};
use std::path::Path;

fn corpus() -> GuardCorpus {
    toml::from_str(include_str!("../../../../eval/command-guard/cases.toml")).unwrap()
}

#[test]
fn audit_scratch_targets_do_not_set_outside_evidence() {
    let cases: Vec<_> = corpus()
        .command
        .into_iter()
        .filter(|c| matches!(c.id.as_str(), "A4" | "A5"))
        .collect();
    assert_eq!(cases.len(), 2);
    for case in cases {
        let (_, outside) = case.judge_command(Path::new("/workspace"));
        assert!(!outside, "{}: scratch is not outside evidence", case.id);
    }
}

#[test]
fn audit_secret_rows_are_owned_by_the_executing_boundary() {
    use crate::engine::command_guard::{
        static_classify_with_paths, CommandSite, SecretPaths, TrustedHeads,
    };
    use crate::llm::tool_names as tn;
    use crate::runtime::agent_sandbox::SandboxMode;
    let policy = lucidos_agent_sandbox::Policy::default();
    for mode in [
        SandboxMode::Enforced,
        SandboxMode::Inherited,
        SandboxMode::OwnerOptedOut,
        SandboxMode::PermittedOutside,
    ] {
        for truncation in [false, true] {
            let paths = SecretPaths::for_policy_with_truncation(&policy, Some(mode), truncation);
            for case in corpus()
                .command
                .into_iter()
                .filter(|c| c.id.starts_with('B'))
            {
                let verdict = static_classify_with_paths(
                    tn::RUN_BASH,
                    &serde_json::json!({"command": case.command}),
                    CommandSite::TextOnly {
                        root: Some(Path::new("/workspace")),
                    },
                    &TrustedHeads::NONE,
                    &paths,
                );
                assert_eq!(
                    matches!(
                        verdict,
                        StaticVerdict::SettledIrreversible(SideEffectCategory::SecretRead)
                    ),
                    mode != SandboxMode::Enforced || !truncation,
                    "{}: {mode:?}, truncation={truncation}: {verdict:?}",
                    case.id
                );
                // Detection remains available to steer a kernel refusal.
                assert!(paths.command(&case.command), "{}", case.id);
            }
        }
    }
}

fn choice(value: &str) -> Answer {
    Answer::Choice(ChoiceAnswer {
        choice: value.into(),
        probabilities: [(value.into(), 1.0)].into_iter().collect(),
        confidence: 1.0,
    })
}

#[test]
fn corpus_floor_refuses_in_every_sandbox_mode() {
    use crate::engine::command_guard::{
        static_classify_with_paths, CommandSite, SecretPaths, TrustedHeads,
    };
    use crate::llm::tool_names as tn;
    use crate::runtime::agent_sandbox::{policy_from, PolicyInputs, SandboxMode};
    let policy = policy_from(&PolicyInputs {
        lucidos_home: None,
        data_dirs: vec![],
        workspaces: vec![],
    });
    for mode in [
        SandboxMode::Enforced,
        SandboxMode::Inherited,
        SandboxMode::OwnerOptedOut,
        SandboxMode::PermittedOutside,
    ] {
        let paths = SecretPaths::for_policy(&policy, Some(mode));
        for case in corpus()
            .command
            .into_iter()
            .filter(|c| c.lane == "catastrophic")
        {
            let verdict = static_classify_with_paths(
                tn::RUN_BASH,
                &serde_json::json!({"command": case.command}),
                CommandSite::TextOnly {
                    root: Some(Path::new("/workspace")),
                },
                &TrustedHeads::NONE,
                &paths,
            );
            let StaticVerdict::Settled(lane) = verdict else {
                panic!("{}: {verdict:?}", case.id)
            };
            assert_eq!(
                action_for_lane(lane, None, None),
                GuardAction::Refuse,
                "{}",
                case.id
            );
        }
    }
}

#[test]
fn standing_corpus_covers_audit_and_seed_cases() {
    let corpus = corpus();
    let ids: std::collections::BTreeSet<_> = corpus.command.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids.len(), corpus.command.len(), "duplicate case id");
    assert_eq!(
        ids.iter().filter(|id| id.starts_with("legacy-")).count(),
        61
    );
    for (prefix, count) in [("A", 5), ("B", 13)] {
        for n in 1..=count {
            assert!(ids.contains(format!("{prefix}{n}").as_str()));
        }
    }
}

#[test]
fn standing_corpus_replays_static_judge_and_channel_actions() {
    for case in corpus().command {
        let verdict = case.static_verdict(Path::new("/workspace"));
        let answers = Answers::new(
            [
                (LANE.into(), choice(&case.lane)),
                (
                    CATEGORY.into(),
                    choice(case.category.as_deref().unwrap_or("none")),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let stub = command_judge_questions::read(&answers, &case.command);
        let (lane, category) = match verdict {
            StaticVerdict::Settled(lane) => (lane, None),
            StaticVerdict::SettledIrreversible(cat) => (RiskLane::IrreversibleDanger, Some(cat)),
            StaticVerdict::NeedsJudge(_) => (stub.lane, stub.category),
        };
        match case.lane.as_str() {
            "safe" => assert_eq!(lane, RiskLane::Safe, "{}", case.id),
            "reversible" => assert!(
                matches!(lane, RiskLane::Safe | RiskLane::ReversibleDanger),
                "{}",
                case.id
            ),
            "irreversible" => assert_eq!(lane, RiskLane::IrreversibleDanger, "{}", case.id),
            "catastrophic" => assert_eq!(lane, RiskLane::Catastrophic, "{}", case.id),
            unknown => panic!("unknown lane {unknown}"),
        }
        if matches!(case.id.as_str(), "A4" | "A5" | "write-then-run") {
            assert_eq!(
                action_for_lane(lane, None, category),
                GuardAction::Proceed,
                "{}",
                case.id
            );
        }
        let expected = match lane {
            RiskLane::Safe => GuardAction::Proceed,
            RiskLane::ReversibleDanger => GuardAction::Checkpoint,
            RiskLane::Catastrophic => GuardAction::Refuse,
            RiskLane::IrreversibleDanger => GuardAction::Ask,
        };
        assert_eq!(
            action_for_lane(lane, None, category),
            expected,
            "{}",
            case.id
        );
        let trigger = action_for_lane(lane, Some(EventChannel::Trigger), category);
        if lane == RiskLane::IrreversibleDanger {
            assert_eq!(
                trigger,
                GuardAction::FailTrigger(category.unwrap_or(SideEffectCategory::Other)),
                "{}",
                case.id
            );
        } else {
            assert_eq!(trigger, expected, "{}", case.id);
        }
    }
}
