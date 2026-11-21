/// The command each generated file's header names.
pub const REGENERATE: &str = "./scripts/codegen.sh";

/// A string as a JSON (and so TypeScript) literal.
pub fn quoted(text: &str) -> String {
    serde_json::to_string(text).expect("a string serialises")
}
