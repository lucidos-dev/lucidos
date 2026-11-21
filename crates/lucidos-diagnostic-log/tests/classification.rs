#[test]
fn log_values_require_explicit_classification() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/fail_*.rs");
    cases.pass("tests/ui/pass_*.rs");
}
