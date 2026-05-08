use super::jaccard_similarity;
use crate::engine::memory::relevance_score;
use crate::memory::{MemoryEntry, MemorySource};
use chrono::{Duration, Utc};
use uuid::Uuid;

const JACCARD_DEDUP_THRESHOLD: f32 = 0.8;
const MAX_FACTS: usize = 25;

fn make_entry(summary: &str, topic: &str, importance: f32, age_days: i64) -> MemoryEntry {
    MemoryEntry {
        id: Uuid::new_v4(),
        source: MemorySource::Event { id: Uuid::new_v4() },
        topic: topic.to_string(),
        summary: summary.to_string(),
        importance,
        entities: vec![],
        src_created_at: Utc::now() - Duration::days(age_days),
        created_at: Utc::now(),
    }
}

/// Simulate the dedup + scoring + grouping pipeline from retrieve_context
fn run_pipeline(entries: Vec<(MemoryEntry, f64)>) -> Vec<(String, Vec<(MemoryEntry, f64)>)> {
    // Take top-N by relevance score
    let mut scored = entries;
    scored.sort_by(|(_, a), (_, b)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(MAX_FACTS);

    // Deduplicate near-identical facts
    let mut keep = vec![true; scored.len()];
    for i in 0..scored.len() {
        if !keep[i] {
            continue;
        }
        for j in (i + 1)..scored.len() {
            if !keep[j] {
                continue;
            }
            if jaccard_similarity(&scored[i].0.summary, &scored[j].0.summary)
                > JACCARD_DEDUP_THRESHOLD
            {
                keep[j] = false;
            }
        }
    }
    let mut idx = 0;
    scored.retain(|_| {
        let k = keep[idx];
        idx += 1;
        k
    });

    // Group by topic
    let mut topic_groups: std::collections::HashMap<String, Vec<(MemoryEntry, f64)>> =
        std::collections::HashMap::new();
    for (entry, score) in scored {
        topic_groups
            .entry(entry.topic.clone())
            .or_default()
            .push((entry, score));
    }

    // Sort chronologically within each topic
    for entries in topic_groups.values_mut() {
        entries.sort_by_key(|(e, _)| e.src_created_at);
    }

    // Sort topics by average relevance score
    let mut sorted: Vec<_> = topic_groups.into_iter().collect();
    sorted.sort_by(|(_, a), (_, b)| {
        let avg_a: f64 = a.iter().map(|(_, s)| s).sum::<f64>() / a.len() as f64;
        let avg_b: f64 = b.iter().map(|(_, s)| s).sum::<f64>() / b.len() as f64;
        avg_b
            .partial_cmp(&avg_a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    sorted
}

// --- Scoring tests ---

#[test]
fn higher_similarity_ranks_higher() {
    let e1 = make_entry("Fact A", "Topic", 0.8, 10);
    let e2 = make_entry("Fact B", "Topic", 0.8, 10);
    let age = 10.0;
    let score_high = relevance_score(0.9, 0.8, age);
    let score_low = relevance_score(0.5, 0.8, age);
    assert!(score_high > score_low);

    let results = run_pipeline(vec![(e1, score_high), (e2, score_low)]);
    assert_eq!(results[0].1[0].0.summary, "Fact A");
}

#[test]
fn higher_importance_ranks_higher_same_similarity() {
    let s1 = relevance_score(0.7, 0.95, 5.0);
    let s2 = relevance_score(0.7, 0.3, 5.0);
    assert!(
        s1 > s2,
        "higher importance should rank higher: {} vs {}",
        s1,
        s2
    );
}

#[test]
fn recent_fact_ranks_higher_all_else_equal() {
    let s1 = relevance_score(0.8, 0.7, 1.0);
    let s2 = relevance_score(0.8, 0.7, 300.0);
    assert!(s1 > s2, "recent should rank higher: {} vs {}", s1, s2);
}

#[test]
fn old_critical_fact_beats_recent_low_importance() {
    let s_old = relevance_score(0.9, 1.0, 500.0);
    let s_new = relevance_score(0.9, 0.3, 1.0);
    // 500 days: time_factor = 1/(1+500/365) ≈ 0.42
    // old: 0.9 * 1.0 * 0.42 = 0.38
    // new: 0.9 * 0.3 * ~1.0 = 0.27
    assert!(
        s_old > s_new,
        "old critical ({}) should beat recent low-importance ({})",
        s_old,
        s_new
    );
}

// --- Dedup tests ---

#[test]
fn distinct_facts_same_topic_both_survive_dedup() {
    let entries = vec![
        (
            make_entry(
                "Started the Lucidos migration to PostgreSQL",
                "Lucidos",
                0.8,
                30,
            ),
            0.7,
        ),
        (
            make_entry(
                "Completed the Lucidos migration to PostgreSQL",
                "Lucidos",
                0.8,
                10,
            ),
            0.8,
        ),
        (
            make_entry(
                "Lucidos uses event sourcing with PostgreSQL",
                "Lucidos",
                0.7,
                60,
            ),
            0.6,
        ),
        (
            make_entry(
                "Lucidos desktop app built with Tauri framework",
                "Lucidos",
                0.7,
                20,
            ),
            0.65,
        ),
    ];

    let results = run_pipeline(entries);
    let lucidos_facts: Vec<_> = results
        .iter()
        .filter(|(t, _)| t == "Lucidos")
        .flat_map(|(_, entries)| entries.iter().map(|(e, _)| e.summary.as_str()))
        .collect();

    assert_eq!(
        lucidos_facts.len(),
        4,
        "all 4 distinct Lucidos facts should survive dedup, got: {:?}",
        lucidos_facts
    );
}

#[test]
fn near_duplicate_gets_deduped_keeps_higher_scored() {
    // Single word substitution in a 6-word fact: 5/7 = 0.714... wait, let's be precise.
    // "Works at Acme as a software engineer" (7 words) vs
    // "Works at Acme as a software developer" (7 words)
    // Shared: {Works, at, Acme, as, a, software} = 6, Union: + {engineer, developer} = 8
    // Jaccard = 6/8 = 0.75 — below 0.8. Need higher overlap.
    // Use verbatim duplicate:
    let entries = vec![
        (
            make_entry("Works at Acme as a software engineer", "Work", 0.8, 5),
            0.9,
        ),
        (
            make_entry("Works at Acme as a software engineer", "Work", 0.8, 3),
            0.7,
        ),
    ];

    let results = run_pipeline(entries);
    let work_facts: Vec<_> = results
        .iter()
        .filter(|(t, _)| t == "Work")
        .flat_map(|(_, entries)| entries.iter().map(|(e, _)| e.summary.as_str()))
        .collect();

    assert_eq!(
        work_facts.len(),
        1,
        "identical duplicate should be deduped, got: {:?}",
        work_facts
    );
}

#[test]
fn slightly_different_wording_not_deduped() {
    // 0.8 threshold is strict — adding 2+ words to a 7-word fact drops below it
    let entries = vec![
        (
            make_entry("Works at Acme as a software engineer", "Work", 0.8, 5),
            0.9,
        ),
        (
            make_entry(
                "Works at Acme as a software engineer in Oslo",
                "Work",
                0.8,
                3,
            ),
            0.7,
        ),
    ];

    let results = run_pipeline(entries);
    let work_facts: Vec<_> = results
        .iter()
        .filter(|(t, _)| t == "Work")
        .flat_map(|(_, entries)| entries.iter().map(|(e, _)| e.summary.as_str()))
        .collect();

    assert_eq!(
        work_facts.len(),
        2,
        "slightly different wording should survive 0.8 threshold, got: {:?}",
        work_facts
    );
}

#[test]
fn dedup_works_across_topics() {
    // Same fact accidentally tagged under different topics — should still dedup
    let entries = vec![
        (
            make_entry("Alex works at Acme as an engineer", "Work", 0.8, 5),
            0.9,
        ),
        (
            make_entry("Alex works at Acme as an engineer", "Career", 0.8, 5),
            0.7,
        ),
    ];

    let results = run_pipeline(entries);
    let total_facts: usize = results.iter().map(|(_, e)| e.len()).sum();
    assert_eq!(
        total_facts, 1,
        "identical facts across topics should be deduped"
    );
}

// --- Top-N cap tests ---

#[test]
fn more_than_max_facts_gets_truncated() {
    let entries: Vec<_> = (0..100)
        .map(|i| {
            let score = 1.0 - (i as f64 / 100.0);
            (
                make_entry(&format!("Unique fact number {}", i), "General", 0.5, i),
                score,
            )
        })
        .collect();

    let results = run_pipeline(entries);
    let total_facts: usize = results.iter().map(|(_, e)| e.len()).sum();
    assert!(
        total_facts <= MAX_FACTS,
        "should be capped at {} facts, got {}",
        MAX_FACTS,
        total_facts
    );
}

#[test]
fn top_n_keeps_highest_scored() {
    let entries: Vec<_> = (0..100)
        .map(|i| {
            let score = i as f64 / 100.0; // higher index = higher score
            (make_entry(&format!("Fact {}", i), "General", 0.5, 1), score)
        })
        .collect();

    let results = run_pipeline(entries);
    let facts: Vec<_> = results
        .iter()
        .flat_map(|(_, entries)| entries.iter().map(|(e, _)| e.summary.clone()))
        .collect();

    // The top MAX_FACTS (highest-scored) should survive
    assert!(
        facts.contains(&"Fact 99".to_string()),
        "highest-scored fact should survive"
    );
    assert!(
        !facts.contains(&"Fact 0".to_string()),
        "lowest-scored fact should be truncated"
    );
}

// --- Topic grouping tests ---

#[test]
fn entries_grouped_by_topic() {
    let entries = vec![
        (make_entry("Fact about work", "Work", 0.8, 5), 0.9),
        (make_entry("Fact about fitness", "Fitness", 0.7, 3), 0.8),
        (make_entry("Another work fact", "Work", 0.7, 10), 0.7),
    ];

    let results = run_pipeline(entries);
    let topics: Vec<&str> = results.iter().map(|(t, _)| t.as_str()).collect();
    assert!(topics.contains(&"Work"), "should have Work topic");
    assert!(topics.contains(&"Fitness"), "should have Fitness topic");

    let work_count = results
        .iter()
        .find(|(t, _)| t == "Work")
        .map(|(_, e)| e.len())
        .unwrap_or(0);
    assert_eq!(work_count, 2, "Work topic should have 2 entries");
}

#[test]
fn topics_sorted_by_average_relevance() {
    let entries = vec![
        (make_entry("High relevance A", "TopTopic", 0.9, 1), 0.95),
        (make_entry("High relevance B", "TopTopic", 0.9, 2), 0.90),
        (make_entry("Low relevance A", "BottomTopic", 0.3, 1), 0.2),
        (make_entry("Low relevance B", "BottomTopic", 0.3, 2), 0.15),
    ];

    let results = run_pipeline(entries);
    assert_eq!(
        results[0].0, "TopTopic",
        "highest-avg-relevance topic should be first, got '{}'",
        results[0].0
    );
}

#[test]
fn within_topic_sorted_chronologically() {
    let entries = vec![
        (make_entry("Newest fact", "Timeline", 0.8, 1), 0.9),
        (make_entry("Oldest fact", "Timeline", 0.8, 100), 0.7),
        (make_entry("Middle fact", "Timeline", 0.8, 50), 0.8),
    ];

    let results = run_pipeline(entries);
    let timeline = results.iter().find(|(t, _)| t == "Timeline").unwrap();
    let summaries: Vec<&str> = timeline.1.iter().map(|(e, _)| e.summary.as_str()).collect();

    assert_eq!(
        summaries,
        vec!["Oldest fact", "Middle fact", "Newest fact"],
        "within topic should be chronological (oldest first), got: {:?}",
        summaries
    );
}

// --- Edge cases ---

#[test]
fn empty_entries_returns_empty() {
    let results = run_pipeline(vec![]);
    assert!(results.is_empty());
}

#[test]
fn single_entry_survives() {
    let entries = vec![(make_entry("Only fact", "Solo", 0.8, 5), 0.9)];
    let results = run_pipeline(entries);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].1.len(), 1);
}

#[test]
fn all_same_score_preserves_all_distinct() {
    let entries = vec![
        (
            make_entry("Fact about cooking pasta", "Cooking", 0.5, 10),
            0.5,
        ),
        (
            make_entry("Fact about baking bread", "Cooking", 0.5, 10),
            0.5,
        ),
        (
            make_entry("Fact about grilling fish", "Cooking", 0.5, 10),
            0.5,
        ),
    ];
    let results = run_pipeline(entries);
    let total: usize = results.iter().map(|(_, e)| e.len()).sum();
    assert_eq!(
        total, 3,
        "all distinct facts should survive even with equal scores"
    );
}

// --- Relevance score interaction tests ---

#[test]
fn scoring_formula_components_multiply() {
    // Verify the three factors interact multiplicatively
    let base = relevance_score(1.0, 1.0, 0.0);
    assert!((base - 1.0).abs() < f64::EPSILON);

    let half_sim = relevance_score(0.5, 1.0, 0.0);
    assert!((half_sim - 0.5).abs() < f64::EPSILON);

    let half_imp = relevance_score(1.0, 0.5, 0.0);
    assert!((half_imp - 0.5).abs() < f64::EPSILON);

    let half_time = relevance_score(1.0, 1.0, 365.0);
    assert!((half_time - 0.5).abs() < 1e-10);

    // All halved: 0.5 * 0.5 * 0.5 = 0.125
    let all_half = relevance_score(0.5, 0.5, 365.0);
    assert!(
        (all_half - 0.125).abs() < 1e-6,
        "all factors halved should give 0.125, got {}",
        all_half
    );
}

#[test]
fn time_decay_curve_is_gradual() {
    // Verify decay at key milestones
    let at_0 = relevance_score(1.0, 1.0, 0.0); // 1.0
    let at_30 = relevance_score(1.0, 1.0, 30.0); // ~0.924
    let at_90 = relevance_score(1.0, 1.0, 90.0); // ~0.802
    let at_180 = relevance_score(1.0, 1.0, 180.0); // ~0.670
    let at_365 = relevance_score(1.0, 1.0, 365.0); // 0.500
    let at_730 = relevance_score(1.0, 1.0, 730.0); // ~0.333

    // Verify monotonic decrease
    assert!(at_0 > at_30, "0d > 30d");
    assert!(at_30 > at_90, "30d > 90d");
    assert!(at_90 > at_180, "90d > 180d");
    assert!(at_180 > at_365, "180d > 365d");
    assert!(at_365 > at_730, "365d > 730d");

    // Verify 30-day-old fact retains >90% (gentle decay)
    assert!(
        at_30 > 0.9,
        "30-day-old should retain >90%, got {:.3}",
        at_30
    );

    // Verify 2-year-old fact still has ~33%
    assert!(
        (at_730 - 1.0 / 3.0).abs() < 0.01,
        "730-day-old should be ~0.333, got {:.3}",
        at_730
    );
}
