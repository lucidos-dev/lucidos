use super::*;
use crate::core::{AppKind, AppManager, AppReveal};
use crate::engine::prompt_footprint::{ItemUsage, UseVerdict};

fn app(id: &str, description: &str) -> App {
    App {
        id: id.to_string(),
        name: id.to_string(),
        description: description.to_string(),
        icon: None,
        reveal: AppReveal::OnLoad,
        kind: AppKind::App,
        origin: None,
        reusable: false,
        built_in: false,
        params: Default::default(),
    }
}

fn summary(id: &str, description: &str) -> KnowhowSummary {
    KnowhowSummary {
        id: id.to_string(),
        name: id.to_string(),
        description: description.to_string(),
    }
}

const LIMITS: FootprintLimits = FootprintLimits {
    section_ceiling: 100,
    total_ceiling: 150,
    unused_days: 60,
};

/// Text that appears only because the workspace has items is workspace
/// footprint. With no items, no section may count a char.
#[test]
fn an_empty_workspace_reports_zero_for_every_section() {
    for (spec, section) in build_sections(&FootprintInputs::default()) {
        assert_eq!(section.chars(), 0, "{} counts chars with no items", spec.id);
        assert!(section.items.is_empty(), "{} lists items", spec.id);
    }
}

#[test]
fn every_section_has_a_unique_id() {
    let mut ids: Vec<&str> = SECTIONS.iter().map(|s| s.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), SECTIONS.len());
}

/// The report's sections are the very text the turn's system prompt carries,
/// built from the same files by the same loaders and builders. The Widgets
/// rules and the list's header are engine text, counted in neither.
#[tokio::test]
async fn each_system_prompt_section_is_the_text_the_turn_sends() {
    let bus = crate::test_support::offline_event_bus();
    let tmp = tempfile::tempdir().unwrap();
    let manager = AppManager::new(tmp.path()).unwrap();
    crate::core::apps::tests::app_and_widget(&manager, &bus).await;
    manager
        .set_widget_reusable(&bus, "fare-grid", true, None)
        .await
        .unwrap();
    let intents = vec![Intent {
        id: "weekly-review".to_string(),
        name: "Weekly review".to_string(),
        knowhow: Vec::new(),
        content: String::new(),
    }];
    let knowhow = vec![summary("ops/nightly", "Runs the nightly sweep.")];
    let app_knowhow = vec![("habit-tracker".to_string(), summary("flow", "The flow."))];

    let turn = format!(
        "{}{}{}",
        footprint::apps_section(&manager, true).unwrap(),
        footprint::build_intents_section(&intents),
        footprint::build_knowhow_section(&knowhow, &app_knowhow),
    );

    let (apps, reusable_widgets) = manager.apps_and_reusable_widgets().unwrap();
    let inputs = FootprintInputs {
        apps,
        reusable_widgets,
        knowhow,
        app_knowhow,
        intents,
        ..FootprintInputs::default()
    };
    let mut counted = 0;
    for (spec, section) in build_sections(&inputs) {
        let chars = section.chars();
        let Some(text) = section.text.filter(|t| !t.is_empty()) else {
            continue;
        };
        assert!(
            turn.contains(&text),
            "{} is not what the turn sends:\n{text}\n--- turn ---\n{turn}",
            spec.id
        );
        assert_eq!(chars, text.chars().count());
        counted += chars;
    }
    let engine_text = crate::engine::widget_guidance::widgets_rules(
        crate::engine::widget_guidance::WidgetAudience::Chat { automatic: true },
    )
    .chars()
    .count()
        + crate::engine::widget_guidance::WIDGETS_LIST_HEADER
            .chars()
            .count();
    assert_eq!(
        counted + engine_text,
        turn.chars().count(),
        "every char of the block counts in exactly one footprint"
    );
}

#[test]
fn a_clipped_description_reports_what_the_line_leaves_out() {
    let long = "word ".repeat(100);
    let inputs = FootprintInputs {
        apps: vec![app("long", &long), app("short", "Fits.")],
        ..FootprintInputs::default()
    };
    let section = available_apps(&inputs);
    assert!(
        section.items[0].clipped_chars > 250,
        "{:?}",
        section.items[0]
    );
    assert_eq!(section.items[1].clipped_chars, 0);
}

#[test]
fn the_open_app_section_bills_the_largest_listing_and_names_each_app() {
    let inputs = FootprintInputs {
        open_app_knowhow: vec![
            ("small".to_string(), vec![summary("a", "One.")]),
            (
                "large".to_string(),
                vec![summary("a", "One."), summary("b", "Two.")],
            ),
            ("none".to_string(), Vec::new()),
        ],
        ..FootprintInputs::default()
    };
    let section = open_app_knowhow(&inputs);
    let large = footprint::build_app_knowhow_listing(
        "large",
        &[summary("a", "One."), summary("b", "Two.")],
    );
    assert_eq!(section.text.as_deref(), Some(large.as_str()));
    let ids: Vec<&str> = section.items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, ["small", "large"]);
}

#[test]
fn assembly_flags_each_ceiling_and_judges_apps() {
    let long = "x".repeat(150);
    let inputs = FootprintInputs {
        apps: vec![app("a", &long)],
        open_app_knowhow: vec![("a".to_string(), vec![summary("k", "K.")])],
        ..FootprintInputs::default()
    };
    let report = assemble(
        build_sections(&inputs),
        &LIMITS,
        |item| {
            item.usage = Some(ItemUsage {
                last_used_days_ago: None,
                verdict: UseVerdict::NotYetJudged,
            });
        },
        vec![SystemPromptArea {
            label: "body",
            chars: 7,
        }],
    );
    let apps = report
        .sections
        .iter()
        .find(|s| s.id == "available-apps")
        .unwrap();
    assert!(apps.over_ceiling);
    assert!(apps.items[0].usage.is_some());
    let open_app = report
        .sections
        .iter()
        .find(|s| s.id == "open-app-knowhow")
        .unwrap();
    assert_eq!(open_app.items[0].kind, ItemKind::AppKnowhow);
    assert_eq!(
        report.total_chars,
        report.sections.iter().map(|s| s.chars).sum::<usize>()
    );
    assert!(report.over_total_ceiling);
    assert_eq!(report.system_prompt_chars, 7);
}

#[test]
fn a_knowhow_id_resolves_to_its_file_in_load_order() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("knowhow");
    let apps = tmp.path().join("apps");
    std::fs::create_dir_all(local.join("ops")).unwrap();
    std::fs::create_dir_all(apps.join("habit-tracker/knowhow")).unwrap();
    std::fs::write(local.join("ops/nightly.md"), "x").unwrap();
    std::fs::write(apps.join("habit-tracker/knowhow/flow.md"), "x").unwrap();
    let dirs = KnowhowDirs {
        shared: None,
        local: local.clone(),
        apps: Some(apps.clone()),
        triggers: None,
    };
    assert_eq!(
        knowhow_doc_path(&dirs, "ops/nightly"),
        Some(local.join("ops/nightly.md"))
    );
    assert_eq!(
        knowhow_doc_path(&dirs, "habit-tracker/flow"),
        Some(apps.join("habit-tracker/knowhow/flow.md"))
    );
    assert_eq!(knowhow_doc_path(&dirs, "../escape"), None);
    assert_eq!(knowhow_doc_path(&dirs, "missing"), None);
}

/// `execute_intent` reads these docs with no `load_knowhow` call, so the
/// unused check must count them as used.
#[test]
fn an_intent_loads_its_knowhow_ids_and_its_apps_whole_folder() {
    let intents = vec![
        Intent {
            id: "weekly-review".to_string(),
            name: "Weekly review".to_string(),
            knowhow: vec!["ops/nightly".to_string()],
            content: String::new(),
        },
        Intent {
            id: "habit-tracker/log".to_string(),
            name: "Log a habit".to_string(),
            knowhow: Vec::new(),
            content: String::new(),
        },
    ];
    let loads = IntentKnowhow::of(&intents);
    assert!(loads.loads("ops/nightly", false));
    assert!(loads.loads("habit-tracker/flow", true));
    assert!(
        !loads.loads("habit-tracker/flow", false),
        "a workspace group doc sharing the app's name is not the app's"
    );
    assert!(!loads.loads("ops/other", false));
    assert!(!loads.loads("documents/flow", true));
}

#[test]
fn an_open_app_listing_is_its_own_kind_named_after_the_app() {
    let inputs = FootprintInputs {
        apps: vec![App {
            name: "Habit Tracker".to_string(),
            ..app("habit-tracker", "Tracks habits.")
        }],
        open_app_knowhow: vec![(
            "habit-tracker".to_string(),
            vec![summary("flow", "The flow.")],
        )],
        ..FootprintInputs::default()
    };
    let item = &open_app_knowhow(&inputs).items[0];
    assert_eq!(item.kind, ItemKind::AppKnowhow);
    assert_eq!(item.name, "Habit Tracker");
}
