mod guard_tests {
    #[test]
    fn nothing_emits_a_retired_allow_once_event() {
        let retired = [
            "ThreadEvent::OwnerApprovalRequested {",
            "ThreadEvent::OwnerApprovalSpent {",
            "owner_approval: Some(",
        ];
        let reads = |line: &str| line.contains("{ .. }");
        let mut hits = Vec::new();
        for (rel, text) in crate::test_support::source_scan::production_sources() {
            for line in text.lines().filter(|l| !reads(l)) {
                if retired.iter().any(|needle| line.contains(needle)) {
                    hits.push(format!("{rel}: {}", line.trim()));
                }
            }
        }
        assert!(hits.is_empty(), "retired emits remain: {hits:#?}");
    }
}
