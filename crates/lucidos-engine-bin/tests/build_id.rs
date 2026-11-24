// The test runs the same source hashing code as Cargo's build script.
#[allow(
    dead_code,
    reason = "Cargo's build-script entry point is outside this test target"
)]
#[path = "../build.rs"]
mod build_script;
