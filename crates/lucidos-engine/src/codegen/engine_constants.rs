//! The SDK's copy of the engine constants a client must agree with. Generated,
//! so no client restates one (ADR 0368).

use std::fmt::Display;

use crate::api::actor::HEADER_DEVICE_ID;
use crate::api::frontend_snapshot::SOURCE_COMMIT_FILE;
use crate::api::internal::CLIENT_LOG_MAX_BATCH;
use crate::api::presence_pong::PresenceOutcome;
use crate::api::proxy_timeout::CLIENT_WAIT_SECS;
use crate::api::threads::archive_all::MAX_IDS;
use crate::api::{SSE_KEEPALIVE_EVENT, SSE_KEEPALIVE_INTERVAL};
use crate::core::apps::WidgetSize;
use crate::core::preference_catalog::MEMORY_MODULE_WHILE_UNSET;
use crate::engine::agent_recovery::{
    AUTO_RECOVERY_AFTER_HANG_REASON, AUTO_RESUME_AFTER_API_ERROR_REASON,
    AUTO_RESUME_AFTER_SWITCH_REASON, EFFORT_RESTART_REASON, ENGINE_RESTART_INTERRUPT_REASON,
    HARDEN_REQUESTED_REASON, USER_CLICKED_CONTINUE_REASON,
};
use crate::engine::agent_session::side_question::SIDE_QUESTION_TIMEOUT;
use crate::engine::command_guard::{
    BENIGN_PREFIX_WORDS, GRANTS_NOTHING_HEADS, GUARD_SHELLS, MAX_SHELL_LAYERS,
};
use crate::engine::summary_tree::log::EntryKind;
use crate::engine::thread_events::{KEEP_ALIVE_ENDING_EVENT_TYPES, KEEP_ALIVE_IDLE_REASON};
use crate::engine::thread_triage::archive_all::{PINNED_SUB_THREAD, SAME_FAMILY};
use crate::engine::widgets::WIDGET_PARAMS_MAX_BYTES;
use crate::engine::TURN_RUNNING_REASON;
use crate::llm::judgment::endpoint::{
    SystemOneEndpoint, CLOUDFLARE_ACCOUNTS_PREFIX, CLOUDFLARE_WORKERS_AI_CREDENTIAL_SERVICE,
    OPENROUTER_JEV_MODEL, SYSTEM_ONE_CUSTOM_CREDENTIAL_SERVICE, TYPESAFE_API_KEY_ENV,
    TYPESAFE_CREDENTIAL_SERVICE,
};
use crate::llm::judgment::WAKE_DECISION_DEFAULT;
use crate::voice::sections::SECTIONS;

use super::{quoted, REGENERATE};

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

pub(super) fn render() -> String {
    let mut out = String::new();
    out.push_str("// AUTO-GENERATED. Do not edit by hand.\n");
    out.push_str(&format!("// Regenerate: {REGENERATE}\n"));
    out.push_str("//\n");
    out.push_str("// Source of truth: the engine constant each export names, under\n");
    out.push_str("// crates/lucidos-engine/src.\n\n");
    let mut ts = Writer { out, file: "" };

    ts.from("api/internal.rs");
    ts.export("CLIENT_LOG_MAX_BATCH", CLIENT_LOG_MAX_BATCH);
    ts.from("api/history.rs");
    let keepalive_ms = SSE_KEEPALIVE_INTERVAL.as_millis();
    ts.renamed(
        "SSE_KEEPALIVE_INTERVAL",
        "SSE_KEEPALIVE_INTERVAL_MS",
        keepalive_ms,
    );
    ts.export("SSE_KEEPALIVE_EVENT", quoted(SSE_KEEPALIVE_EVENT));
    ts.from("engine/widgets.rs");
    ts.export("WIDGET_PARAMS_MAX_BYTES", WIDGET_PARAMS_MAX_BYTES);
    ts.from("core/apps.rs");
    let sizes = list(&WidgetSize::ALL.map(WidgetSize::as_str));
    ts.renamed("WidgetSize::ALL", "WIDGET_SIZES", sizes);
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
    ts.from("core/preference_catalog.rs");
    ts.export(
        "MEMORY_MODULE_WHILE_UNSET",
        format!("{} as const", quoted(MEMORY_MODULE_WHILE_UNSET)),
    );
    ts.from("api/frontend_snapshot.rs");
    ts.export("SOURCE_COMMIT_FILE", quoted(SOURCE_COMMIT_FILE));
    ts.from("engine/chat/rung.rs");
    ts.export("TURN_RUNNING_REASON", quoted(TURN_RUNNING_REASON));

    ts.from("engine/command_guard.rs");
    ts.export("GUARD_SHELLS", list(&GUARD_SHELLS));
    ts.export("BENIGN_PREFIX_WORDS", list(&BENIGN_PREFIX_WORDS));
    ts.export("GRANTS_NOTHING_HEADS", list(&GRANTS_NOTHING_HEADS));
    ts.export("MAX_SHELL_LAYERS", MAX_SHELL_LAYERS);

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
        ("EFFORT_RESTART_REASON", EFFORT_RESTART_REASON),
    ];
    for (name, value) in reasons {
        ts.export(name, quoted(value));
    }

    ts.from("engine/thread_events/event_impl.rs");
    ts.export("KEEP_ALIVE_IDLE_REASON", quoted(KEEP_ALIVE_IDLE_REASON));
    let ending = list(KEEP_ALIVE_ENDING_EVENT_TYPES);
    ts.export("KEEP_ALIVE_ENDING_EVENT_TYPES", ending);

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
        ("OPENROUTER_JEV_MODEL", OPENROUTER_JEV_MODEL),
    ];
    for (name, value) in judgment {
        ts.export(name, quoted(value));
    }
    let ids = list(&SystemOneEndpoint::ALL.map(SystemOneEndpoint::id));
    ts.renamed("SystemOneEndpoint::id", "SYSTEM_ONE_ENDPOINT_IDS", ids);
    ts.from("llm/judgment/select.rs");
    ts.renamed(
        "WAKE_DECISION_DEFAULT",
        "WAKE_DECISION_DEFAULT_ENDPOINT",
        quoted(WAKE_DECISION_DEFAULT.id()),
    );

    ts.from("engine/summary_tree/log.rs");
    let kinds = list(&EntryKind::ALL.map(|k| k.as_str()));
    ts.renamed("EntryKind::as_str", "ENTRY_KINDS", kinds);

    ts.from("api/presence_pong.rs");
    let outcomes =
        PresenceOutcome::BY_STRENGTH.map(|o| serde_json::to_value(o).expect("a unit variant"));
    let outcomes = outcomes.each_ref().map(|v| v.as_str().expect("a string"));
    ts.renamed(
        "PresenceOutcome::BY_STRENGTH",
        "PRESENCE_OUTCOMES_BY_STRENGTH",
        list(&outcomes),
    );

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
