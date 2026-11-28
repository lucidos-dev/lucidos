use super::relevance_score;

#[test]
fn perfect_match_today_high_importance() {
    let score = relevance_score(1.0, 1.0, 0.0);
    assert!(
        (score - 1.0).abs() < f64::EPSILON,
        "perfect match today should be 1.0, got {}",
        score
    );
}

#[test]
fn zero_similarity_means_zero_relevance() {
    let score = relevance_score(0.0, 1.0, 0.0);
    assert!(
        score.abs() < f64::EPSILON,
        "zero similarity should give zero relevance, got {}",
        score
    );
}

#[test]
fn zero_importance_means_zero_relevance() {
    let score = relevance_score(0.8, 0.0, 0.0);
    assert!(
        score.abs() < f64::EPSILON,
        "zero importance should give zero relevance, got {}",
        score
    );
}

#[test]
fn one_year_old_halves_time_factor() {
    let today = relevance_score(1.0, 1.0, 0.0);
    let one_year = relevance_score(1.0, 1.0, 365.0);
    assert!(
        (one_year - today * 0.5).abs() < 1e-10,
        "1 year old should be half of today's score: today={}, 1yr={}",
        today,
        one_year
    );
}

#[test]
fn recent_high_importance_beats_old_low_importance() {
    let recent = relevance_score(0.7, 0.9, 1.0);
    let old = relevance_score(0.7, 0.35, 300.0);
    assert!(
        recent > old,
        "recent high-importance ({}) should beat old low-importance ({})",
        recent,
        old
    );
}

#[test]
fn very_relevant_old_fact_can_still_rank_high() {
    let old_critical = relevance_score(1.0, 1.0, 730.0);
    let recent_meh = relevance_score(0.5, 0.5, 1.0);
    assert!(
        old_critical > recent_meh,
        "old critical fact ({}) should still beat recent mediocre ({})",
        old_critical,
        recent_meh
    );
}

#[test]
fn importance_scales_linearly() {
    let low = relevance_score(0.8, 0.3, 10.0);
    let high = relevance_score(0.8, 0.9, 10.0);
    assert!(
        (high / low - 3.0).abs() < 1e-6,
        "3x importance should give 3x score, got ratio {}",
        high / low
    );
}

#[test]
fn negative_age_treated_as_zero() {
    let score = relevance_score(1.0, 1.0, -5.0);
    assert!(
        score <= 1.0 + f64::EPSILON,
        "negative age should not boost score above 1.0, got {}",
        score
    );
}
