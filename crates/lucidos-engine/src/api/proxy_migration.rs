//! Startup migration: rewrite legacy six-variant `apis.json` configs into
//! the `Vec<AuthLayer>` pipeline shape, with a timestamped backup.
//!
//! Triggered once at engine startup before proxy config is loaded. The
//! contract is "make legacy operators upgrade-on-restart" — no
//! configuration knob, no opt-out.
//!
//! The legacy `ProxyAuth` enum is gone, so the only paths that produce a
//! pipeline-shape `apis.json` are this migration (one-time per workspace)
//! and manual editing. Idempotent: already-pipeline configs pass through.

use serde_json::{json, Value};
use std::path::Path;

/// Outcome of `migrate_apis_json_if_needed`. Used by the engine startup
/// log line to indicate whether anything happened.
#[derive(Debug, Clone)]
pub enum MigrationOutcome {
    /// File doesn't exist, so no proxies are configured. No-op.
    NotPresent,
    /// The live file was left alone. Either no provider carries a legacy
    /// `auth` block, or none of the legacy blocks can be upgraded. The
    /// config load reports whatever it then refuses, by name.
    Unchanged,
    /// File was rewritten; backup created at `backup_path`.
    Migrated { backup_path: std::path::PathBuf },
}

/// True iff at least one provider in the file uses the legacy
/// `auth.type` shape. Any `auth.type` value counts here, including an
/// unknown or removed one like `credential_bundle`. Rejecting those is
/// the translator's job, and that is where the operator-actionable
/// error originates.
///
/// **The question is asked per provider, never per file.** A file is a
/// mixture whenever anything appends to it: the workspace agent writes
/// new entries beside old ones, one shape at a time. Reading a single
/// pipeline sibling as "the whole file is migrated" leaves every legacy
/// entry beside it to fail the next parse (ADR 0135).
/// `translate_legacy_in_place` skips the pipeline entries itself, so
/// answering per provider costs those entries nothing.
pub fn is_legacy_shape(json: &Value) -> bool {
    let Some(map) = json.as_object() else {
        return false;
    };
    map.values().any(|provider| {
        provider
            .get("auth")
            .is_some_and(|auth| auth.get("pipeline").is_none() && auth.get("type").is_some())
    })
}

/// Translate every provider's `auth` block from the legacy 6-variant
/// shape to the pipeline shape, in place. Returns whether anything
/// changed.
///
/// **One untranslatable provider never costs the others their upgrade.**
/// An unknown or removed type (`credential_bundle`, say) leaves that one
/// entry byte-identical and the rest are still rewritten. Nothing is
/// silently dropped: the entry stays legacy, so the config load refuses
/// it by name and `legacy_rejection` supplies the actionable sentence.
pub fn translate_legacy_in_place(json: &mut Value) -> bool {
    let Some(map) = json.as_object_mut() else {
        return false;
    };
    let mut changed = false;
    for (provider_name, provider) in map.iter_mut() {
        let Some(auth) = provider.get_mut("auth") else {
            continue;
        };
        if auth.get("pipeline").is_some() {
            // Already migrated, so leave it alone.
            continue;
        }
        if let Ok(translated) = translate_auth_block(provider_name, auth) {
            *auth = translated;
            changed = true;
        }
    }
    changed
}

/// Why the engine refuses this provider's `auth` block, as a sentence an
/// operator can act on. `None` when the block is not legacy-shaped, and
/// so is somebody else's problem to describe.
///
/// This is the load's half of the translator. A legacy block reaching
/// the load is one the migration could not rewrite, so the translator's
/// own rejection is the right words. The fallback arm covers the block
/// that WOULD translate: the file was edited after boot, and no
/// migration has run over it yet.
pub fn legacy_rejection(provider_name: &str, auth: &Value) -> Option<String> {
    if auth.get("pipeline").is_some() || auth.get("type").is_none() {
        return None;
    }
    Some(
        translate_auth_block(provider_name, auth)
            .err()
            .unwrap_or_else(|| {
                format!(
                    "provider '{provider_name}': auth uses the removed legacy shape; \
                     restart the workspace to upgrade it automatically"
                )
            }),
    )
}

fn translate_auth_block(provider_name: &str, auth: &Value) -> Result<Value, String> {
    let auth_type = auth
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("provider '{provider_name}': auth missing `type` field"))?;

    let layer = match auth_type {
        "bearer" => {
            let credential = require_string(provider_name, auth, "credential")?;
            json!({
                "type": "static_credential",
                "kind": "bearer",
                "credential": credential,
            })
        }
        "api_key" => {
            let credential = require_string(provider_name, auth, "credential")?;
            let mut obj = json!({
                "type": "static_credential",
                "kind": "api_key",
                "credential": credential,
            });
            if let Some(header) = auth.get("header").and_then(|v| v.as_str()) {
                obj.as_object_mut()
                    .unwrap()
                    .insert("header".into(), Value::String(header.to_string()));
            }
            obj
        }
        "basic" => {
            let credential = require_string(provider_name, auth, "credential")?;
            json!({
                "type": "static_credential",
                "kind": "basic",
                "credential": credential,
            })
        }
        "query_param" => {
            let credential = require_string(provider_name, auth, "credential")?;
            let param_name = require_string(provider_name, auth, "param_name")?;
            json!({
                "type": "static_credential",
                "kind": "query_param",
                "credential": credential,
                "param_name": param_name,
            })
        }
        "hmac_signed" => {
            // Pass the entire auth block through with `type: hmac_signed`
            // — the pipeline keeps `hmac_signed` as a first-class layer
            // (A future WasmSigner-backed binance-style signer can replace it.)
            auth.clone()
        }
        "script_handshake" => {
            let script = require_string(provider_name, auth, "script")?;
            let mut obj = json!({
                "type": "script_handshake",
                "script": script,
            });
            // `credential` is optional (a handshake script may source its
            // secret elsewhere — OS keychain, OAuth-only exchange). Absent or
            // null → carry nothing through; a string → carry it; a
            // present-but-non-string value is a malformed config → fail loudly
            // rather than silently drop it (matches the pipeline deserializer,
            // which rejects a non-string `credential`).
            match auth.get("credential") {
                None | Some(Value::Null) => {}
                Some(Value::String(credential)) => {
                    obj.as_object_mut()
                        .unwrap()
                        .insert("credential".into(), Value::String(credential.clone()));
                }
                Some(_) => {
                    return Err(format!(
                        "provider '{provider_name}': auth.credential must be a string"
                    ))
                }
            }
            obj
        }
        // Deliberate negative guard: `credential_bundle` was removed
        // long before this migration shipped. We refuse it here so an
        // operator who keeps a stale `apis.json` around sees an
        // actionable error instead of silent 4xx at request time.
        "credential_bundle" => {
            return Err(format!(
                "provider '{provider_name}': auth.type = 'credential_bundle' was removed; \
                 split it into per-credential entries (one provider per credential), \
                 then re-run the engine"
            ))
        }
        other => {
            return Err(format!(
                "provider '{provider_name}': unknown auth.type '{other}'; \
                 expected one of bearer/api_key/basic/query_param/hmac_signed/script_handshake"
            ))
        }
    };

    Ok(json!({
        "pipeline": [layer],
    }))
}

fn require_string(provider_name: &str, auth: &Value, field: &str) -> Result<String, String> {
    auth.get(field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            format!("provider '{provider_name}': auth missing required string field '{field}'")
        })
}

/// Top-level entry point called from engine startup. If `apis.json`
/// exists and holds a legacy entry the translator understands, copy the
/// file to a timestamped backup, rewrite the live file in place, and
/// return `Migrated { backup_path }`.
///
/// An `Err` here means the filesystem or the JSON parser said no, never
/// that a provider is misconfigured. The caller must not treat it as
/// fatal: the config load runs next and reports every entry it refuses.
pub fn migrate_apis_json_if_needed(workspace_path: &Path) -> Result<MigrationOutcome, String> {
    let path = workspace_path.join("data/config/apis.json");
    if !path.exists() {
        return Ok(MigrationOutcome::NotPresent);
    }
    let content = std::fs::read_to_string(&path).map_err(|e| format!("read apis.json: {e}"))?;
    let mut json: Value =
        serde_json::from_str(&content).map_err(|e| format!("parse apis.json: {e}"))?;
    if !is_legacy_shape(&json) {
        return Ok(MigrationOutcome::Unchanged);
    }

    // Translate in memory FIRST. It touches no file, so nothing needs
    // protecting until we know there is a rewrite to protect against.
    //
    // Order matters, and taking the backup first was a bug. The stamp holds
    // whole seconds and `fs::copy` overwrites. A file whose only legacy
    // entry is untranslatable took a copy on every boot and deleted it
    // again. Two boots inside one second then overwrote a real backup with
    // post-migration content, and removed it.
    if !translate_legacy_in_place(&mut json) {
        return Ok(MigrationOutcome::Unchanged);
    }
    let rewritten =
        serde_json::to_string_pretty(&json).map_err(|e| format!("serialize apis.json: {e}"))?;

    // Now there is a rewrite coming, so copy the original beside it.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let backup_path = path.with_extension(format!("json.bak.{ts}"));
    std::fs::copy(&path, &backup_path).map_err(|e| format!("backup apis.json: {e}"))?;

    // Atomic write: temp file + rename. A failure of either leaves the live
    // file untouched, which makes the backup a copy of a file that never
    // changed. Drop it, or an unwritable `data/config` leaks one per boot
    // now that this no longer aborts the boot.
    let tmp_path = path.with_extension("json.tmp");
    let write_then_rename = std::fs::write(&tmp_path, rewritten)
        .map_err(|e| format!("write tmp apis.json: {e}"))
        .and_then(|()| {
            std::fs::rename(&tmp_path, &path)
                .map_err(|e| format!("rename tmp apis.json -> live: {e}"))
        });
    if let Err(e) = write_then_rename {
        let _ = std::fs::remove_file(&tmp_path);
        let _ = std::fs::remove_file(&backup_path);
        return Err(e);
    }

    Ok(MigrationOutcome::Migrated { backup_path })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_apis_json(workspace: &Path, contents: &str) {
        let dir = workspace.join("data/config");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("apis.json"), contents).unwrap();
    }

    fn read_apis_json(workspace: &Path) -> Value {
        let s = std::fs::read_to_string(workspace.join("data/config/apis.json")).unwrap();
        serde_json::from_str(&s).unwrap()
    }

    /// Run the migration over a config holding an untranslatable provider,
    /// then return the sentence the config load will refuse it with.
    ///
    /// Asserts the shape the whole design rests on: the migration itself
    /// never errors on a bad provider, and leaves that entry legacy so the
    /// load is the one place that reports it.
    fn refusal_after_migration(workspace: &Path, provider: &str) -> String {
        migrate_apis_json_if_needed(workspace).expect("a bad provider is not a migration error");
        let json = read_apis_json(workspace);
        let auth = &json[provider]["auth"];
        assert!(
            auth.get("pipeline").is_none(),
            "an untranslatable entry must be left legacy, not half-rewritten"
        );
        legacy_rejection(provider, auth).expect("a legacy block the load must refuse")
    }

    #[test]
    fn missing_file_is_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        match migrate_apis_json_if_needed(tmp.path()).unwrap() {
            MigrationOutcome::NotPresent => {}
            other => panic!("expected NotPresent, got {other:?}"),
        }
    }

    #[test]
    fn already_pipeline_shape_is_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        write_apis_json(
            tmp.path(),
            r#"{
              "binance": {
                "base_url": "https://api.binance.com",
                "auth": {"pipeline": [
                  {"type": "wasm_signer", "module": "binance-hmac"}
                ]}
              }
            }"#,
        );
        match migrate_apis_json_if_needed(tmp.path()).unwrap() {
            MigrationOutcome::Unchanged => {}
            other => panic!("expected Unchanged, got {other:?}"),
        }
        // Backup file must NOT have been created.
        let any_backup = std::fs::read_dir(tmp.path().join("data/config"))
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains(".bak."));
        assert!(
            !any_backup,
            "no backup should be made for already-migrated configs"
        );
    }

    #[test]
    fn a_legacy_entry_beside_pipeline_entries_still_migrates() {
        // The wedge this migration exists to prevent, and the one it missed. A
        // workspace appends one entry in the old shape to a file whose other
        // entries are already pipeline-shape. Judging the file by its first
        // pipeline sibling leaves the legacy entry to fail the next parse.
        let tmp = tempfile::tempdir().unwrap();
        write_apis_json(
            tmp.path(),
            r#"{
              "github": {
                "base_url": "https://api.github.com",
                "auth": {"pipeline": [
                  {"type": "script_handshake", "script": "scripts/auth/gh.py"}
                ]}
              },
              "jira": {
                "base_url": "https://example.atlassian.net",
                "auth": {"type": "basic", "credential": "jira"}
              },
              "frankfurter": {"base_url": "https://api.frankfurter.dev/v1"}
            }"#,
        );
        match migrate_apis_json_if_needed(tmp.path()).unwrap() {
            MigrationOutcome::Migrated { backup_path } => {
                assert!(backup_path.exists(), "backup should be on disk");
            }
            other => panic!("expected Migrated, got {other:?}"),
        }
        let migrated = read_apis_json(tmp.path());
        let jira = &migrated["jira"]["auth"]["pipeline"][0];
        assert_eq!(jira["type"], "static_credential");
        assert_eq!(jira["kind"], "basic");
        assert_eq!(jira["credential"], "jira");
        // The already-migrated sibling is untouched, and an auth-less entry
        // stays auth-less.
        let github = &migrated["github"]["auth"]["pipeline"][0];
        assert_eq!(github["type"], "script_handshake");
        assert_eq!(github["script"], "scripts/auth/gh.py");
        assert!(migrated["frankfurter"].get("auth").is_none());
        // Still idempotent: nothing legacy is left to rewrite.
        match migrate_apis_json_if_needed(tmp.path()).unwrap() {
            MigrationOutcome::Unchanged => {}
            other => panic!("second run: expected Unchanged, got {other:?}"),
        }
    }

    #[test]
    fn legacy_bearer_is_translated_to_static_credential() {
        let tmp = tempfile::tempdir().unwrap();
        write_apis_json(
            tmp.path(),
            r#"{
              "comfort": {
                "base_url": "https://accsmart.panasonic.com",
                "auth": {"type": "bearer", "credential": "comfort-cloud"}
              }
            }"#,
        );
        match migrate_apis_json_if_needed(tmp.path()).unwrap() {
            MigrationOutcome::Migrated { backup_path } => {
                assert!(backup_path.exists(), "backup should be on disk");
            }
            other => panic!("expected Migrated, got {other:?}"),
        }
        let migrated = read_apis_json(tmp.path());
        let layer = &migrated["comfort"]["auth"]["pipeline"][0];
        assert_eq!(layer["type"], "static_credential");
        assert_eq!(layer["kind"], "bearer");
        assert_eq!(layer["credential"], "comfort-cloud");
    }

    #[test]
    fn legacy_api_key_with_custom_header_preserves_header() {
        let tmp = tempfile::tempdir().unwrap();
        write_apis_json(
            tmp.path(),
            r#"{"x": {"base_url": "https://x", "auth": {
              "type": "api_key", "credential": "k", "header": "X-API-Key"
            }}}"#,
        );
        migrate_apis_json_if_needed(tmp.path()).unwrap();
        let migrated = read_apis_json(tmp.path());
        let layer = &migrated["x"]["auth"]["pipeline"][0];
        assert_eq!(layer["kind"], "api_key");
        assert_eq!(layer["header"], "X-API-Key");
    }

    #[test]
    fn legacy_query_param_is_translated_with_param_name() {
        let tmp = tempfile::tempdir().unwrap();
        write_apis_json(
            tmp.path(),
            r#"{"x": {"base_url": "https://x", "auth": {
              "type": "query_param", "credential": "k", "param_name": "api-key"
            }}}"#,
        );
        migrate_apis_json_if_needed(tmp.path()).unwrap();
        let migrated = read_apis_json(tmp.path());
        let layer = &migrated["x"]["auth"]["pipeline"][0];
        assert_eq!(layer["kind"], "query_param");
        assert_eq!(layer["param_name"], "api-key");
    }

    #[test]
    fn legacy_hmac_signed_is_passed_through_as_first_class_layer() {
        let tmp = tempfile::tempdir().unwrap();
        write_apis_json(
            tmp.path(),
            r#"{"binance": {"base_url": "https://api.binance.com", "auth": {
              "type": "hmac_signed",
              "key_credential": "binance-key",
              "secret_credential": "binance-secret",
              "key_header": "X-MBX-APIKEY",
              "algorithm": "sha256",
              "signed_payload": "query_string",
              "signature_param": "signature",
              "timestamp_param": "timestamp"
            }}}"#,
        );
        migrate_apis_json_if_needed(tmp.path()).unwrap();
        let migrated = read_apis_json(tmp.path());
        let layer = &migrated["binance"]["auth"]["pipeline"][0];
        assert_eq!(layer["type"], "hmac_signed");
        assert_eq!(layer["key_credential"], "binance-key");
        assert_eq!(layer["secret_credential"], "binance-secret");
    }

    #[test]
    fn legacy_script_handshake_is_translated() {
        let tmp = tempfile::tempdir().unwrap();
        write_apis_json(
            tmp.path(),
            r#"{"comfort-cloud": {"base_url": "https://accsmart.panasonic.com", "auth": {
              "type": "script_handshake",
              "credential": "comfort-cloud",
              "script": "scripts/auth/comfort-cloud.py"
            }}}"#,
        );
        migrate_apis_json_if_needed(tmp.path()).unwrap();
        let migrated = read_apis_json(tmp.path());
        let layer = &migrated["comfort-cloud"]["auth"]["pipeline"][0];
        assert_eq!(layer["type"], "script_handshake");
        assert_eq!(layer["credential"], "comfort-cloud");
        assert_eq!(layer["script"], "scripts/auth/comfort-cloud.py");
    }

    #[test]
    fn legacy_script_handshake_without_credential_is_translated() {
        // `credential` is optional for script_handshake — a legacy block that
        // omits it must migrate cleanly, with no `credential` key in the
        // pipeline layer (script sources its secret elsewhere).
        let tmp = tempfile::tempdir().unwrap();
        write_apis_json(
            tmp.path(),
            r#"{"keychain-api": {"base_url": "https://api.example.com", "auth": {
              "type": "script_handshake",
              "script": "scripts/auth/keychain-login.py"
            }}}"#,
        );
        migrate_apis_json_if_needed(tmp.path()).unwrap();
        let migrated = read_apis_json(tmp.path());
        let layer = &migrated["keychain-api"]["auth"]["pipeline"][0];
        assert_eq!(layer["type"], "script_handshake");
        assert_eq!(layer["script"], "scripts/auth/keychain-login.py");
        assert!(
            layer.get("credential").is_none(),
            "no credential key should be emitted when the legacy block omitted it"
        );
    }

    #[test]
    fn legacy_script_handshake_with_non_string_credential_is_rejected() {
        // A present-but-non-string `credential` is a malformed config — the
        // migration must fail loudly, not silently drop the field (which would
        // migrate the layer to a credential-less one that runs the script with
        // no CRED_* injection).
        let tmp = tempfile::tempdir().unwrap();
        write_apis_json(
            tmp.path(),
            r#"{"x": {"base_url": "https://x", "auth": {
              "type": "script_handshake",
              "credential": 123,
              "script": "scripts/auth/x.py"
            }}}"#,
        );
        let err = refusal_after_migration(tmp.path(), "x");
        assert!(
            err.contains("credential") && err.contains("string"),
            "error should name the malformed credential field: {err}"
        );
    }

    #[test]
    fn migration_rewrites_live_file_atomically() {
        let tmp = tempfile::tempdir().unwrap();
        write_apis_json(
            tmp.path(),
            r#"{"x": {"base_url": "https://x", "auth": {"type": "bearer", "credential": "k"}}}"#,
        );
        migrate_apis_json_if_needed(tmp.path()).unwrap();
        // Post-migration: live file is pipeline-shape AND a .tmp file from
        // the atomic-rename path is NOT left around.
        let migrated = read_apis_json(tmp.path());
        assert!(migrated["x"]["auth"]["pipeline"].is_array());
        let tmp_left = tmp.path().join("data/config/apis.json.tmp");
        assert!(
            !tmp_left.exists(),
            "atomic rename should leave no .tmp file"
        );
    }

    #[test]
    fn re_running_migration_after_first_run_is_idempotent_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        write_apis_json(
            tmp.path(),
            r#"{"x": {"base_url": "https://x", "auth": {"type": "bearer", "credential": "k"}}}"#,
        );
        // First run migrates.
        match migrate_apis_json_if_needed(tmp.path()).unwrap() {
            MigrationOutcome::Migrated { .. } => {}
            other => panic!("first run: expected Migrated, got {other:?}"),
        }
        // Second run: file is now pipeline-shape → no-op.
        match migrate_apis_json_if_needed(tmp.path()).unwrap() {
            MigrationOutcome::Unchanged => {}
            other => panic!("second run: expected Unchanged, got {other:?}"),
        }
    }

    #[test]
    fn migration_rejects_removed_credential_bundle_type_with_actionable_error() {
        // Negative-test guard: an operator who keeps an old
        // credential_bundle entry around must see a loud, actionable
        // error instead of a silent 4xx at request time.
        let tmp = tempfile::tempdir().unwrap();
        write_apis_json(
            tmp.path(),
            r#"{"old": {"base_url": "https://x", "auth": {"type": "credential_bundle"}}}"#,
        );
        let err = refusal_after_migration(tmp.path(), "old");
        assert!(
            err.contains("credential_bundle"),
            "error should name the removed type: {err}"
        );
        assert!(
            err.contains("removed"),
            "error should make the rejection explicit: {err}"
        );
    }

    #[test]
    fn a_refused_provider_does_not_block_its_translatable_neighbour() {
        // The migration is per provider on the way out as well as on the
        // way in. One entry nothing can translate must not cost the entry
        // beside it its upgrade.
        let tmp = tempfile::tempdir().unwrap();
        write_apis_json(
            tmp.path(),
            r#"{
              "old": {"base_url": "https://x", "auth": {"type": "credential_bundle"}},
              "good": {"base_url": "https://y", "auth": {"type": "bearer", "credential": "k"}}
            }"#,
        );
        match migrate_apis_json_if_needed(tmp.path()).unwrap() {
            MigrationOutcome::Migrated { .. } => {}
            other => panic!("expected Migrated, got {other:?}"),
        }
        let migrated = read_apis_json(tmp.path());
        assert_eq!(migrated["good"]["auth"]["pipeline"][0]["kind"], "bearer");
        assert_eq!(migrated["old"]["auth"]["type"], "credential_bundle");
    }

    #[test]
    fn a_file_of_only_refused_providers_is_left_alone_with_no_backup() {
        // Nothing translates, so no rewrite is coming and no copy is taken.
        // This runs TWICE on purpose. `is_legacy_shape` stays true forever
        // while an untranslatable entry sits in the file, so every boot
        // reaches this path. Backing up before the translate decision made
        // that a copy-and-delete per boot. Two boots in one second could
        // then overwrite a real backup with post-migration content.
        let tmp = tempfile::tempdir().unwrap();
        write_apis_json(
            tmp.path(),
            r#"{"old": {"base_url": "https://x", "auth": {"type": "credential_bundle"}}}"#,
        );
        let before = std::fs::read_to_string(tmp.path().join("data/config/apis.json")).unwrap();
        for boot in 1..=2 {
            match migrate_apis_json_if_needed(tmp.path()).unwrap() {
                MigrationOutcome::Unchanged => {}
                other => panic!("boot {boot}: expected Unchanged, got {other:?}"),
            }
            let any_backup = std::fs::read_dir(tmp.path().join("data/config"))
                .unwrap()
                .filter_map(|e| e.ok())
                .any(|e| e.file_name().to_string_lossy().contains(".bak."));
            assert!(!any_backup, "boot {boot}: no backup for an unchanged file");
        }
        let after = std::fs::read_to_string(tmp.path().join("data/config/apis.json")).unwrap();
        assert_eq!(before, after, "the live file must be byte-identical");
    }

    #[test]
    fn a_translatable_block_read_before_any_migration_says_to_restart() {
        // The runtime-edit case: somebody appends a legacy entry to a live
        // workspace's file. It would translate fine, so the refusal must
        // not claim it is unsupported.
        let auth = serde_json::json!({"type": "bearer", "credential": "k"});
        let reason = legacy_rejection("x", &auth).expect("a legacy block");
        assert!(
            reason.contains("restart"),
            "should say what fixes it: {reason}"
        );
    }

    #[test]
    fn a_pipeline_block_is_nobody_to_refuse() {
        let auth = serde_json::json!({"pipeline": []});
        assert!(legacy_rejection("x", &auth).is_none());
    }

    #[test]
    fn invalid_hmac_algorithm_is_rejected_during_pipeline_load() {
        // The hmac_signed legacy block is passed through verbatim into
        // the new pipeline shape, so the rejection happens at the next
        // load step (load_proxy_config -> serde_json deserialization of
        // PipelineConfig -> HmacAlgorithm). Verify migration succeeds AND
        // the rewritten file fails to deserialize through load_proxy_config.
        let tmp = tempfile::tempdir().unwrap();
        write_apis_json(
            tmp.path(),
            r#"{"binance": {"base_url": "https://api.binance.com", "auth": {
              "type": "hmac_signed",
              "key_credential": "binance-key",
              "secret_credential": "binance-secret",
              "algorithm": "md5",
              "signed_payload": "query_string"
            }}}"#,
        );
        // Migration itself doesn't validate algorithm strings — they pass
        // through into the new shape. The next load step rejects them.
        migrate_apis_json_if_needed(tmp.path()).unwrap();
        let load = crate::api::proxy::load_proxy_config(tmp.path());
        assert!(load.providers.is_empty(), "the entry must not be served");
        let load_err = &load.rejected.first().expect("a refusal").reason;
        assert!(
            load_err.contains("md5") || load_err.contains("algorithm"),
            "load_proxy_config should reject unknown hmac algorithm, got: {load_err}"
        );
    }

    #[test]
    fn unknown_auth_type_is_rejected_with_named_error() {
        let tmp = tempfile::tempdir().unwrap();
        write_apis_json(
            tmp.path(),
            r#"{"y": {"base_url": "https://y", "auth": {"type": "magic-handshake"}}}"#,
        );
        assert!(refusal_after_migration(tmp.path(), "y").contains("magic-handshake"));
    }

    #[test]
    fn missing_required_field_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        // bearer requires `credential`.
        write_apis_json(
            tmp.path(),
            r#"{"x": {"base_url": "https://x", "auth": {"type": "bearer"}}}"#,
        );
        assert!(refusal_after_migration(tmp.path(), "x").contains("credential"));
    }
}
