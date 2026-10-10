//! The SDK's copy of the engine constants a client must agree with. Generated,
//! so no client restates one (ADR 0368).

use std::fmt::Display;
use std::path::PathBuf;

use crate::api::actor::HEADER_DEVICE_ID;
use crate::api::internal::CLIENT_LOG_MAX_BATCH;
use crate::api::proxy_timeout::CLIENT_WAIT_SECS;
use crate::api::threads::archive_all::MAX_IDS;
use crate::engine::agent_recovery::{
    AUTO_RECOVERY_AFTER_HANG_REASON, AUTO_RESUME_AFTER_API_ERROR_REASON,
    AUTO_RESUME_AFTER_SWITCH_REASON, ENGINE_RESTART_INTERRUPT_REASON, HARDEN_REQUESTED_REASON,
    USER_CLICKED_CONTINUE_REASON,
};
use crate::engine::agent_session::side_question::SIDE_QUESTION_TIMEOUT;
use crate::engine::claude_code::{
    BROAD_ALLOW_INEFFECTIVE, CC_PROTECTED_PATH_MARKERS, CODEX_BACKEND_TOOLS, SESSION_PATH_TOOLS,
};
use crate::engine::thread_triage::archive_all::{PINNED_SUB_THREAD, SAME_FAMILY};
use crate::engine::widgets::WIDGET_PARAMS_MAX_BYTES;
use crate::llm::judgment::endpoint::{
    SystemOneEndpoint, CLOUDFLARE_ACCOUNTS_PREFIX, CLOUDFLARE_WORKERS_AI_CREDENTIAL_SERVICE,
    SYSTEM_ONE_CUSTOM_CREDENTIAL_SERVICE, TYPESAFE_API_KEY_ENV, TYPESAFE_CREDENTIAL_SERVICE,
};
use crate::runtime::TakesEffect;
use crate::voice::sections::SECTIONS;

const REGENERATE: &str =
    "cargo test -p lucidos-engine --lib generate_engine_constants_file -- --ignored";

fn ts_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/lucidos-sdk/src/generated/engine-constants.ts")
}

fn quoted(text: &str) -> String {
    serde_json::to_string(text).expect("a string serialises")
}

fn list(values: &[&str]) -> String {
    let quoted: Vec<String> = values.iter().map(|v| quoted(v)).collect();
    format!("[{}] as const", quoted.join(", "))
}

/// The file being written, and the engine source file its next exports copy.
struct Writer {
    out: String,
    file: &'static str,
}

impl Writer {
    fn from(&mut self, file: &'static str) {
        self.file = file;
    }

    fn export(&mut self, name: &str, value: impl Display) {
        self.renamed(name, name, value);
    }

    /// An export whose TS name differs from the engine constant it copies.
    fn renamed(&mut self, source: &str, name: &str, value: impl Display) {
        self.out.push_str(&format!(
            "/** `{source}` in `{}`. */\nexport const {name} = {value};\n\n",
            self.file
        ));
    }
}

fn generate_ts() -> String {
    let mut out = String::new();
    out.push_str("// AUTO-GENERATED. Do not edit by hand.\n");
    out.push_str(&format!("// Regenerate: {REGENERATE}\n"));
    out.push_str("//\n");
    out.push_str("// Source of truth: the engine constant each export names, under\n");
    out.push_str("// crates/lucidos-engine/src.\n\n");
    let mut ts = Writer { out, file: "" };

    ts.from("api/internal.rs");
    ts.export("CLIENT_LOG_MAX_BATCH", CLIENT_LOG_MAX_BATCH);
    ts.from("engine/widgets.rs");
    ts.export("WIDGET_PARAMS_MAX_BYTES", WIDGET_PARAMS_MAX_BYTES);
    ts.from("api/threads/archive_all.rs");
    ts.renamed("MAX_IDS", "ARCHIVE_ALL_MAX_IDS", MAX_IDS);
    ts.from("engine/thread_triage/archive_all.rs");
    ts.renamed(
        "SAME_FAMILY",
        "ARCHIVE_ALL_SAME_FAMILY",
        quoted(SAME_FAMILY),
    );
    let pinned = quoted(PINNED_SUB_THREAD);
    ts.renamed("PINNED_SUB_THREAD", "ARCHIVE_ALL_PINNED_SUB_THREAD", pinned);
    ts.from("engine/agent_session/side_question.rs");
    let side_ms = SIDE_QUESTION_TIMEOUT.as_millis();
    ts.renamed("SIDE_QUESTION_TIMEOUT", "SIDE_QUESTION_TIMEOUT_MS", side_ms);
    ts.from("api/proxy_timeout.rs");
    ts.renamed(
        "CLIENT_WAIT_SECS",
        "PROXY_CLIENT_WAIT_SECS",
        CLIENT_WAIT_SECS,
    );
    ts.from("api/actor.rs");
    ts.export("HEADER_DEVICE_ID", quoted(HEADER_DEVICE_ID));

    ts.from("engine/claude_code/mod.rs");
    ts.export("BROAD_ALLOW_INEFFECTIVE", list(BROAD_ALLOW_INEFFECTIVE));
    ts.export("CC_PROTECTED_PATH_MARKERS", list(CC_PROTECTED_PATH_MARKERS));
    ts.export("SESSION_PATH_TOOLS", list(SESSION_PATH_TOOLS));
    ts.export("CODEX_BACKEND_TOOLS", list(CODEX_BACKEND_TOOLS));

    ts.from("engine/agent_recovery/helpers.rs");
    let reasons = [
        ("USER_CLICKED_CONTINUE_REASON", USER_CLICKED_CONTINUE_REASON),
        (
            "AUTO_RECOVERY_AFTER_HANG_REASON",
            AUTO_RECOVERY_AFTER_HANG_REASON,
        ),
        (
            "AUTO_RESUME_AFTER_SWITCH_REASON",
            AUTO_RESUME_AFTER_SWITCH_REASON,
        ),
        (
            "AUTO_RESUME_AFTER_API_ERROR_REASON",
            AUTO_RESUME_AFTER_API_ERROR_REASON,
        ),
        (
            "ENGINE_RESTART_INTERRUPT_REASON",
            ENGINE_RESTART_INTERRUPT_REASON,
        ),
        ("HARDEN_REQUESTED_REASON", HARDEN_REQUESTED_REASON),
    ];
    for (name, value) in reasons {
        ts.export(name, quoted(value));
    }

    ts.from("llm/judgment/endpoint.rs");
    let judgment = [
        ("TYPESAFE_API_KEY_ENV", TYPESAFE_API_KEY_ENV),
        ("TYPESAFE_CREDENTIAL_SERVICE", TYPESAFE_CREDENTIAL_SERVICE),
        (
            "CLOUDFLARE_WORKERS_AI_CREDENTIAL_SERVICE",
            CLOUDFLARE_WORKERS_AI_CREDENTIAL_SERVICE,
        ),
        (
            "SYSTEM_ONE_CUSTOM_CREDENTIAL_SERVICE",
            SYSTEM_ONE_CUSTOM_CREDENTIAL_SERVICE,
        ),
        ("CLOUDFLARE_ACCOUNTS_PREFIX", CLOUDFLARE_ACCOUNTS_PREFIX),
    ];
    for (name, value) in judgment {
        ts.export(name, quoted(value));
    }
    let ids = list(&SystemOneEndpoint::ALL.map(SystemOneEndpoint::id));
    ts.renamed("SystemOneEndpoint::id", "SYSTEM_ONE_ENDPOINT_IDS", ids);

    ts.from("runtime/agent_runtime.rs");
    let takes_effect = TakesEffect::ALL.map(|t| serde_json::to_value(t).expect("a unit variant"));
    let takes_effect = takes_effect
        .each_ref()
        .map(|v| v.as_str().expect("a string"));
    ts.renamed("TakesEffect", "TAKES_EFFECT_VALUES", list(&takes_effect));

    let mut out = ts.out;
    out.push_str("/** `SECTIONS` in `voice/sections.rs`. */\n");
    out.push_str("export const VOICE_RESIDENT_SECTIONS = [\n");
    for section in SECTIONS {
        out.push_str(&format!(
            "  {{ id: {}, title: {} }},\n",
            quoted(section.id),
            quoted(section.title)
        ));
    }
    out.push_str("] as const;\n");
    out
}

#[test]
fn generated_engine_constants_are_up_to_date() {
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
fn generate_engine_constants_file() {
    let path = ts_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, generate_ts()).unwrap();
    crate::log!("[Codegen] wrote {}", path.display());
}
