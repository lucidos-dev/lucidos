use super::jaccard_similarity;

#[test]
fn identical_strings() {
    assert!((jaccard_similarity("hello world", "hello world") - 1.0).abs() < f32::EPSILON);
}

#[test]
fn no_overlap() {
    assert!(jaccard_similarity("hello world", "foo bar").abs() < f32::EPSILON);
}

#[test]
fn partial_overlap() {
    // "hello world foo" vs "hello world bar" → intersection={"hello","world"}, union={"hello","world","foo","bar"}
    let sim = jaccard_similarity("hello world foo", "hello world bar");
    assert!((sim - 0.5).abs() < f32::EPSILON);
}

#[test]
fn empty_strings() {
    assert!((jaccard_similarity("", "") - 1.0).abs() < f32::EPSILON);
}

#[test]
fn high_overlap_above_threshold() {
    // 4 out of 5 words shared → 0.8
    let sim = jaccard_similarity(
        "the user works on projects daily",
        "the user works on projects weekly",
    );
    assert!(sim > 0.7);
}

// --- Tests for the user's concern: distinct same-topic facts must NOT be deduped ---

#[test]
fn started_vs_completed_same_project_survives_dedup() {
    // "Started X" vs "Completed X" — distinct lifecycle events
    let sim = jaccard_similarity(
        "Started the Lucidos migration to PostgreSQL",
        "Completed the Lucidos migration to PostgreSQL",
    );
    assert!(
        sim <= 0.8,
        "started vs completed should survive 0.8 dedup threshold, got {:.3}",
        sim
    );
}

#[test]
fn same_person_different_meetings_survives_dedup() {
    let sim = jaccard_similarity(
        "Meeting with Sarah about the Q2 roadmap",
        "Meeting with Sarah about the Q3 roadmap",
    );
    assert!(
        sim <= 0.8,
        "different meetings should survive dedup, got {:.3}",
        sim
    );
}

#[test]
fn same_project_different_versions_survives_dedup() {
    let sim = jaccard_similarity(
        "Deployed Lucidos v2.1 to production",
        "Deployed Lucidos v2.2 to production",
    );
    assert!(
        sim <= 0.8,
        "different versions should survive dedup, got {:.3}",
        sim
    );
}

#[test]
fn same_system_different_tech_survives_dedup() {
    let sim = jaccard_similarity(
        "Uses Rust for the Lucidos engine backend",
        "Uses TypeScript for the Lucidos frontend",
    );
    assert!(
        sim <= 0.8,
        "different tech choices should survive dedup, got {:.3}",
        sim
    );
}

#[test]
fn same_topic_different_preferences_survives_dedup() {
    let sim = jaccard_similarity(
        "Prefers dark mode in the Lucidos interface",
        "Prefers vim keybindings in the Lucidos interface",
    );
    assert!(
        sim <= 0.8,
        "different preferences should survive dedup, got {:.3}",
        sim
    );
}

#[test]
fn near_duplicate_with_few_extra_words_below_threshold() {
    // 7 shared out of 9 total = 0.778 — below 0.8 threshold.
    // Adding 2+ words to a 7-word fact drops below dedup.
    // This means the 0.8 threshold is strict, which is conservative (fewer false dedup).
    let sim = jaccard_similarity(
        "Works at Acme as a software engineer",
        "Works at Acme as a software engineer in Oslo",
    );
    assert!(
        sim > 0.7 && sim < 0.8,
        "expected ~0.778 (7/9), got {:.3}",
        sim
    );
}

#[test]
fn verbatim_duplicate_gets_deduped() {
    let sim = jaccard_similarity(
        "Started the habit tracker skill project",
        "Started the habit tracker skill project",
    );
    assert!(
        (sim - 1.0).abs() < f32::EPSILON,
        "identical facts should have Jaccard 1.0, got {:.3}",
        sim
    );
}

#[test]
fn single_word_swap_in_short_fact_survives_dedup() {
    // 5 shared out of 7 union = 5/7 ≈ 0.714 — below 0.8 threshold.
    // This shows the 0.8 threshold is conservative: even a single word swap
    // in a 6-word fact drops below it. Only near-verbatim duplicates are removed.
    let sim = jaccard_similarity(
        "Started the habit tracker skill project",
        "Started the habit tracker skill development",
    );
    assert!(
        sim < 0.8,
        "single word swap should survive 0.8 threshold, got {:.3}",
        sim
    );
}

#[test]
fn long_fact_single_word_swap_gets_deduped() {
    // With longer facts, a single word swap is a smaller portion of the total,
    // so the Jaccard stays above 0.8.
    // 11 words, 1 different: 10/12 ≈ 0.833
    let sim = jaccard_similarity(
        "Alex completed the migration of the Lucidos backend to PostgreSQL successfully",
        "Alex completed the migration of the Lucidos backend to PostgreSQL yesterday",
    );
    assert!(
        sim > 0.8,
        "single word swap in 11-word fact should be deduped, got {:.3}",
        sim
    );
}

#[test]
fn completely_different_facts_same_topic_survives() {
    // Both about "Lucidos" topic but completely different facts
    let sim = jaccard_similarity(
        "Lucidos uses event sourcing with PostgreSQL",
        "Lucidos desktop app built with Tauri framework",
    );
    assert!(
        sim <= 0.8,
        "different facts about same project should survive dedup, got {:.3}",
        sim
    );
}

#[test]
fn similar_structure_different_entities_survives() {
    // Same sentence structure, different entities
    let sim = jaccard_similarity(
        "Had a meeting with Alex about the migration plan",
        "Had a meeting with Sarah about the deployment strategy",
    );
    assert!(
        sim <= 0.8,
        "same structure but different entities should survive dedup, got {:.3}",
        sim
    );
}
