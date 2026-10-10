//! The frontend's copy of the catalog, generated so a client never writes a
//! preference default or value list by hand (ADR 0368).

use std::path::PathBuf;

use super::*;

const REGENERATE: &str =
    "cargo test -p lucidos-engine --lib generate_preference_catalog_file -- --ignored";

fn ts_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/lucidos-sdk/src/generated/preference-catalog.ts")
}

fn quoted(text: &str) -> String {
    serde_json::to_string(text).expect("a string serialises")
}

/// What a client reads: settings only. Engine bookkeeping never reaches it.
fn client_specs() -> impl Iterator<Item = &'static PrefSpec> {
    CATALOG
        .iter()
        .copied()
        .filter(|spec| !matches!(spec.access, PrefAccess::Engine { .. }))
}

/// `theme-mode` is exported as `PREF_THEME_MODE`.
fn export_name(key: &str) -> String {
    format!("PREF_{}", key.to_ascii_uppercase().replace('-', "_"))
}

fn type_name(value: PrefValue) -> &'static str {
    match value {
        PrefValue::Bool => "flag",
        PrefValue::Number { .. } => "number",
        PrefValue::Enum(_) => "enum",
        PrefValue::IanaTimezone
        | PrefValue::Text
        | PrefValue::ThemeId
        | PrefValue::ModelBytes
        | PrefValue::ModelEfforts
        | PrefValue::FontFamily => "text",
    }
}

fn generate_ts() -> String {
    let mut out = String::new();
    out.push_str("// AUTO-GENERATED. Do not edit by hand.\n");
    out.push_str(&format!("// Regenerate: {REGENERATE}\n"));
    out.push_str("//\n");
    out.push_str(
        "// Source of truth: CATALOG in crates/lucidos-engine/src/core/preference_catalog.rs.\n",
    );
    out.push_str("// `fallback` is the value an unset preference resolves to, after any\n");
    out.push_str("// inheritance; `null` means it has none.\n\n");
    let flags = |values: &[&str]| {
        values
            .iter()
            .map(|v| quoted(v))
            .collect::<Vec<_>>()
            .join(", ")
    };
    out.push_str("/** The spellings a stored switch reads as on, and as off, in lowercase. */\n");
    out.push_str(&format!(
        "export const FLAG_ON_VALUES = [{}] as const;\n",
        flags(FLAG_ON_VALUES)
    ));
    out.push_str(&format!(
        "export const FLAG_OFF_VALUES = [{}] as const;\n\n",
        flags(FLAG_OFF_VALUES)
    ));
    out.push_str(concat!(
        "/** A stored switch as the engine reads it (`prefs::parse_flag`): on, off, or\n",
        " *  null for a spelling it reads as neither. Case-insensitive. */\n",
        "export function parseFlag(raw: string): boolean | null {\n",
        "  const spelled = raw.trim().toLowerCase();\n",
        "  if ((FLAG_ON_VALUES as readonly string[]).includes(spelled)) return true;\n",
        "  if ((FLAG_OFF_VALUES as readonly string[]).includes(spelled)) return false;\n",
        "  return null;\n",
        "}\n\n",
    ));
    out.push_str("// One export per key, so a bundle imports only what it reads.\n\n");
    for spec in client_specs() {
        out.push_str(&format!("export const {} = {{\n", export_name(spec.key)));
        out.push_str(&format!("  key: {},\n", quoted(spec.key)));
        out.push_str(&format!(
            "  scope: {},\n",
            quoted(match spec.scope {
                PrefScope::Global => "global",
                PrefScope::Device => "device",
            })
        ));
        out.push_str(&format!("  type: {},\n", quoted(type_name(spec.value))));
        match spec.value {
            PrefValue::Enum(values) => {
                let list: Vec<String> = values.iter().map(|v| quoted(v)).collect();
                out.push_str(&format!("  values: [{}],\n", list.join(", ")));
            }
            PrefValue::Number { min, max } => {
                out.push_str(&format!("  min: {min},\n  max: {max},\n"));
            }
            _ => {}
        }
        match spec.default {
            PrefDefault::Inherits(other) => {
                out.push_str(&format!("  inherits: {},\n", quoted(other.key)));
            }
            PrefDefault::Value(_) | PrefDefault::Unset(_) => {}
        }
        match spec.default_value() {
            Some(value) => out.push_str(&format!("  fallback: {},\n", quoted(value))),
            None => out.push_str("  fallback: null,\n"),
        }
        out.push_str("} as const;\n\n");
    }
    out.push_str("export const PREFERENCE_CATALOG = {\n");
    for spec in client_specs() {
        out.push_str(&format!(
            "  {}: {},\n",
            quoted(spec.key),
            export_name(spec.key)
        ));
    }
    out.push_str("} as const;\n\n");
    out.push_str("export type PreferenceKey = keyof typeof PREFERENCE_CATALOG;\n\n");
    out.push_str("/** The allowed values of an enum preference, as a union. */\n");
    out.push_str("export type PreferenceValues<K extends PreferenceKey> =\n");
    out.push_str(
        "  (typeof PREFERENCE_CATALOG)[K] extends { values: readonly (infer V)[] } ? V : never;\n",
    );
    out
}

#[test]
fn generated_preference_catalog_is_up_to_date() {
    let path = ts_path();
    match std::fs::read_to_string(&path) {
        Ok(existing) => assert_eq!(
            existing,
            generate_ts(),
            "Generated {} is stale. Run: {REGENERATE}",
            path.display()
        ),
        Err(_) => panic!(
            "Generated file missing at {}. Run: {REGENERATE}",
            path.display()
        ),
    }
}

#[test]
#[ignore]
fn generate_preference_catalog_file() {
    let path = ts_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, generate_ts()).unwrap();
    crate::log!("[Codegen] wrote {}", path.display());
}
