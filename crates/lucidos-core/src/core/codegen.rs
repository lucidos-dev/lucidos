pub const REGENERATE: &str = "./scripts/codegen.sh";

pub fn quoted(text: &str) -> String {
    serde_json::to_string(text).expect("a string serialises")
}
