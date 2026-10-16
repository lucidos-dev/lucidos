use std::sync::{Arc, RwLock};

use super::*;
use crate::engine::aux_purpose::NotServed;
use crate::llm::model_registry::{load_from_db, resolve_route, ProviderKind};
use crate::test_support::{setup_test_db, teardown_test_db};

/// Every purpose that runs on a chat model through [`super::super::AuxCall`].
const CHAT_MODEL_PURPOSES: &[ContextPurpose] = &[
    ContextPurpose::Title,
    ContextPurpose::ChangeSummary,
    ContextPurpose::ImageDescribe,
    ContextPurpose::Memory,
    ContextPurpose::ConversationSummary,
    ContextPurpose::QueryClassification,
    ContextPurpose::MemoryFind,
    ContextPurpose::CommandJudge,
    ContextPurpose::ReadDecision,
    ContextPurpose::WakeDecision,
];

const CHAT_MODEL: &str = "claude-opus-5-5";

/// A model and the provider that serves it.
type Served = (&'static str, ProviderKind);

fn only(kinds: &[ProviderKind]) -> impl Fn(ProviderKind) -> bool + '_ {
    move |kind| kinds.contains(&kind)
}

/// The unset model and the provider serving it, for `purpose` on `kinds`.
fn resolved(
    registry: &ModelRegistry,
    purpose: ContextPurpose,
    kinds: &[ProviderKind],
) -> (String, ModelSource, Option<ProviderKind>) {
    let (model, source) = select(
        None,
        &keep_capable(purpose, recommended_with(purpose, None), registry),
        CHAT_MODEL,
        registry,
        &Reach::configured(kinds),
    );
    let provider = resolve_route(registry, &model, None, only(kinds))
        .ok()
        .map(|route| route.provider);
    (model, source, provider)
}

/// The plan's table, against the registry the migrations leave. A shipped
/// workspace resolves exactly this, so the seeded routes and the list are
/// pinned together.
#[tokio::test]
async fn the_default_follows_the_configured_providers() {
    use ProviderKind::*;
    let (pool, db_name) = setup_test_db().await;
    let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
    let gemini = "gemini-3-flash-preview";
    let mini = "gpt-5.4-mini";
    let haiku = "claude-haiku-4-5";
    let luna = "gpt-5.6-luna";
    let sol = "gpt-6.1-sol";
    let flash38 = "gemini-3.8-flash";
    let sonnet = "claude-sonnet-5-5";
    // Per install: most purposes (the judge among them), extraction, the
    // summary, the read decision, and the wake decision. The judge's measured
    // default leads the shared fallbacks, so it resolves like the rest. The
    // wake decision runs only on a measured model, so with none reachable it
    // resolves to the chat model, which its gate never asks (`None`).
    type Row<'a> = (
        &'a [ProviderKind],
        Served,
        Served,
        Served,
        Served,
        Option<Served>,
    );
    let rows: &[Row] = &[
        (
            &[Vertex],
            (gemini, Vertex),
            (gemini, Vertex),
            (flash38, Vertex),
            (flash38, Vertex),
            Some((flash38, Vertex)),
        ),
        (
            &ProviderKind::ALL,
            (gemini, Vertex),
            (luna, OpenAi),
            (sol, OpenAi),
            (flash38, Vertex),
            Some((flash38, Vertex)),
        ),
        (
            &[Vertex, Anthropic],
            (gemini, Vertex),
            (gemini, Vertex),
            (flash38, Vertex),
            (flash38, Vertex),
            Some((flash38, Vertex)),
        ),
        (
            &[OpenRouter],
            (gemini, OpenRouter),
            (luna, OpenRouter),
            (sol, OpenRouter),
            (flash38, OpenRouter),
            Some((flash38, OpenRouter)),
        ),
        (
            &[OpenAi],
            (mini, OpenAi),
            (luna, OpenAi),
            (sol, OpenAi),
            (luna, OpenAi),
            Some((sol, OpenAi)),
        ),
        (
            &[Anthropic],
            (haiku, Anthropic),
            (haiku, Anthropic),
            (sonnet, Anthropic),
            (haiku, Anthropic),
            None,
        ),
        (
            &[OpenAi, Anthropic],
            (mini, OpenAi),
            (luna, OpenAi),
            (sol, OpenAi),
            (luna, OpenAi),
            Some((sol, OpenAi)),
        ),
    ];
    for (kinds, most, extraction, summary, read, wake) in rows {
        for purpose in CHAT_MODEL_PURPOSES {
            let expected = match purpose {
                ContextPurpose::Memory => Some(extraction),
                ContextPurpose::ConversationSummary => Some(summary),
                ContextPurpose::ReadDecision => Some(read),
                ContextPurpose::WakeDecision => wake.as_ref(),
                _ => Some(most),
            };
            let (model, source, provider) = resolved(&registry, *purpose, kinds);
            match expected {
                Some((expected, served_by)) => assert_eq!(
                    (model.as_str(), source, provider),
                    (*expected, ModelSource::Default, Some(*served_by)),
                    "{purpose:?} on {kinds:?}"
                ),
                None => assert_eq!(
                    (model.as_str(), source),
                    (CHAT_MODEL, ModelSource::ChatModel),
                    "{purpose:?} on {kinds:?}"
                ),
            }
        }
    }
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// With nothing on the list reachable, the call runs on whatever the
/// workspace chats with. Any install that can chat can run these calls.
#[tokio::test]
async fn with_no_recommended_model_reachable_the_chat_model_runs() {
    use ProviderKind::*;
    let (pool, db_name) = setup_test_db().await;
    let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
    for kinds in [&[Local][..], &[XAi], &[OpenCodeFree]] {
        for purpose in CHAT_MODEL_PURPOSES {
            let (model, source, _) = resolved(&registry, *purpose, kinds);
            assert_eq!(
                (model.as_str(), source),
                (CHAT_MODEL, ModelSource::ChatModel),
                "{purpose:?} on {kinds:?}"
            );
        }
    }
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A pick is honoured or refused, never substituted: a stored model nothing
/// serves stays the model, and reads as unreachable.
#[tokio::test]
async fn a_stored_pick_is_never_substituted() {
    let (pool, db_name) = setup_test_db().await;
    let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
    let kinds = [ProviderKind::Anthropic];
    let (model, source) = select(
        Some("gemini-3-flash-preview".to_string()),
        &recommended_with(ContextPurpose::Title, None),
        CHAT_MODEL,
        &registry,
        &Reach::configured(&kinds),
    );
    assert_eq!(
        (model.as_str(), source),
        ("gemini-3-flash-preview", ModelSource::Preference)
    );
    assert!(!Reach::configured(&kinds).serves(&registry, &model));
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// When Vertex stops serving the judge's default, the judge moves to the next
/// model Vertex serves. The failing call retries on it (I4).
#[tokio::test]
async fn a_not_served_default_moves_to_the_next_reachable_model() {
    let (pool, db_name) = setup_test_db().await;
    let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
    let gemini = "gemini-3-flash-preview";
    let haiku = "claude-haiku-4-5";
    let purpose = ContextPurpose::CommandJudge;
    let list = keep_capable(purpose, recommended_with(purpose, None), &registry);
    let fresh = Reach::configured(&[ProviderKind::Vertex]);
    let retired = Reach::new(
        vec![ProviderKind::Vertex],
        NotServed::of(&[(ProviderKind::Vertex, gemini)]),
    );
    assert_eq!(list[0], gemini);
    // GPT-5.4 mini comes between them, but Vertex does not serve it.
    assert_eq!(
        next_reachable(gemini, &list, &registry, &fresh).as_deref(),
        Some(haiku)
    );
    let (model, source) = select(None, &list, CHAT_MODEL, &registry, &retired);
    assert_eq!((model.as_str(), source), (haiku, ModelSource::Default));
    assert_eq!(
        passed_over(&model, source, &list, &registry, &retired),
        [gemini]
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A stored pick its provider refuses stays the pick and reads unreachable.
/// Moving it would substitute a model the user never chose (I2).
#[tokio::test]
async fn a_not_served_stored_pick_stays_the_pick() {
    let (pool, db_name) = setup_test_db().await;
    let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
    let haiku = "claude-haiku-4-5";
    let purpose = ContextPurpose::CommandJudge;
    let list = recommended_with(purpose, None);
    let retired = Reach::new(
        vec![ProviderKind::Vertex],
        NotServed::of(&[(ProviderKind::Vertex, haiku)]),
    );
    let (model, source) = select(
        Some(haiku.to_string()),
        &list,
        CHAT_MODEL,
        &registry,
        &retired,
    );
    assert_eq!((model.as_str(), source), (haiku, ModelSource::Preference));
    assert!(!retired.serves(&registry, &model));
    assert_eq!(
        passed_over(&model, source, &list, &registry, &retired),
        [haiku]
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Image description skips a candidate that cannot read images, and moves on
/// to the next one a configured provider serves. Other purposes keep it.
#[tokio::test]
async fn image_description_skips_a_candidate_that_cannot_read_images() {
    let (pool, db_name) = setup_test_db().await;
    let mut rows = load_from_db(&pool).await;
    rows.get_mut("gemini-3-flash-preview").unwrap().vision = false;
    let registry: ModelRegistry = Arc::new(RwLock::new(rows));
    let vertex = [ProviderKind::Vertex];
    assert_eq!(
        resolved(&registry, ContextPurpose::ImageDescribe, &vertex),
        (
            "claude-haiku-4-5".to_string(),
            ModelSource::Default,
            Some(ProviderKind::Vertex)
        )
    );
    assert_eq!(
        resolved(&registry, ContextPurpose::Title, &vertex).0,
        "gemini-3-flash-preview"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A text-only env override leads every other list, but never image
/// description's.
#[tokio::test]
async fn a_text_only_override_never_leads_image_description() {
    let (pool, db_name) = setup_test_db().await;
    let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
    let with_env = |purpose| {
        keep_capable(
            purpose,
            recommended_with(purpose, Some("z-ai/glm-5.2")),
            &registry,
        )
    };
    assert_eq!(with_env(ContextPurpose::Title)[0], "z-ai/glm-5.2");
    let images = with_env(ContextPurpose::ImageDescribe);
    assert!(!images.iter().any(|m| m == "z-ai/glm-5.2"), "{images:?}");
    assert_eq!(images[0], "gemini-3-flash-preview");
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A stored pick that cannot read images stays the pick. The call is refused,
/// not moved to a model the user did not choose.
#[tokio::test]
async fn a_stored_text_only_pick_stays_the_pick() {
    let (pool, db_name) = setup_test_db().await;
    let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
    let purpose = ContextPurpose::ImageDescribe;
    let (model, source) = select(
        Some("z-ai/glm-5.2".to_string()),
        &keep_capable(purpose, recommended_with(purpose, None), &registry),
        CHAT_MODEL,
        &registry,
        &Reach::configured(&ProviderKind::ALL),
    );
    assert_eq!(
        (model.as_str(), source),
        ("z-ai/glm-5.2", ModelSource::Preference)
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The env override leads every purpose but the command judge, which never
/// read it, and the wake decision, which runs only on measured models. It
/// goes ahead of a purpose's measured lead too.
#[test]
fn the_extraction_override_leads_all_but_the_judge_and_the_wake_decision() {
    let with_env = |purpose| recommended_with(purpose, Some("gemini-3.8-flash"));
    assert_eq!(with_env(ContextPurpose::Memory)[0], "gemini-3.8-flash");
    assert!(with_env(ContextPurpose::Memory)[1..].starts_with(
        &purpose_lead(ContextPurpose::Memory)
            .iter()
            .map(|m| m.to_string())
            .collect::<Vec<_>>()
    ));
    assert_eq!(with_env(ContextPurpose::Title)[0], "gemini-3.8-flash");
    assert_eq!(
        with_env(ContextPurpose::CommandJudge)[0],
        "gemini-3-flash-preview"
    );
    assert_eq!(
        with_env(ContextPurpose::WakeDecision),
        recommended_with(ContextPurpose::WakeDecision, None)
    );
}

/// Each purpose's measured lead heads its list, then its catalog default, and
/// no model appears twice. The shared fallbacks follow, except on a purpose
/// that runs only on measured models.
#[test]
fn the_lead_then_the_catalog_default_head_the_list_and_nothing_repeats() {
    for purpose in CHAT_MODEL_PURPOSES {
        let list = recommended_with(*purpose, None);
        let pair = model_source(*purpose)
            .prefs()
            .expect("a chat-model purpose");
        let lead = purpose_lead(*purpose);
        assert!(
            list.iter()
                .map(String::as_str)
                .take(lead.len())
                .eq(lead.iter().copied()),
            "{purpose:?}"
        );
        assert_eq!(list[lead.len()], pair.model.default_text(), "{purpose:?}");
        let mut unique = list.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), list.len(), "{purpose:?} repeats a model");
        if measured_only(*purpose) {
            assert_eq!(list.len(), lead.len() + 1, "{purpose:?}: {list:?}");
            continue;
        }
        for fallback in AUX_FALLBACKS {
            assert!(list.iter().any(|m| m == fallback), "{purpose:?}");
        }
    }
}

/// Only a measured-only purpose that fell to the chat model asks no model. A
/// pin and a measured default both run, and every other purpose runs on the
/// chat model as before.
#[test]
fn only_a_measured_only_purpose_on_the_chat_model_asks_no_model() {
    for purpose in CHAT_MODEL_PURPOSES {
        for source in [
            ModelSource::Preference,
            ModelSource::Default,
            ModelSource::ChatModel,
        ] {
            assert_eq!(
                asks_no_model(*purpose, source),
                *purpose == ContextPurpose::WakeDecision && source == ModelSource::ChatModel,
                "{purpose:?} from {source:?}"
            );
        }
    }
}

/// Every fallback has a registry row, so the router can serve it on every
/// backend the row names, and every picker can list it.
#[tokio::test]
async fn every_fallback_has_a_registry_row() {
    let (pool, db_name) = setup_test_db().await;
    let rows = load_from_db(&pool).await;
    for fallback in AUX_FALLBACKS {
        assert!(rows.contains_key(*fallback), "{fallback} has no row");
    }
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The catalog describes the list in prose for the agent and Settings, which
/// cannot import it. Every background row whose model key carries a literal
/// default names its lead, and every fallback it can reach.
#[test]
fn the_description_names_the_lead_and_every_fallback() {
    use crate::core::preference_catalog::PrefDefault;
    for purpose in super::super::BACKGROUND_ROWS {
        let Some(pair) = model_source(*purpose).prefs() else {
            continue;
        };
        if !matches!(pair.model.spec.default, PrefDefault::Value(_)) {
            continue;
        }
        let fallbacks: &[&str] = if measured_only(*purpose) {
            &[]
        } else {
            AUX_FALLBACKS
        };
        for model in purpose_lead(*purpose).iter().chain(fallbacks) {
            assert!(
                pair.model.spec.description.contains(model),
                "{} omits {model}",
                pair.model.key()
            );
        }
    }
}

/// Every lead names a model the registry can route.
#[tokio::test]
async fn every_lead_has_a_registry_row() {
    let (pool, db_name) = setup_test_db().await;
    let rows = load_from_db(&pool).await;
    for purpose in CHAT_MODEL_PURPOSES {
        for model in purpose_lead(*purpose) {
            assert!(
                rows.contains_key(model),
                "{purpose:?} leads with {model}, which has no row"
            );
        }
    }
    pool.close().await;
    teardown_test_db(&db_name).await;
}
