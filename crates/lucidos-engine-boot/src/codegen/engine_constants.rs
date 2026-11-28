use std::fmt::Display;

use lucidos_engine::codegen_inputs::command_guard::{
    grants_nothing_heads, BENIGN_PREFIX_WORDS, GUARD_SHELLS, MAX_SHELL_LAYERS,
};
use lucidos_engine::codegen_inputs::{summary_entry_kinds, SOURCE_COMMIT_FILE};
use lucidos_engine::core::apps::WidgetSize;
use lucidos_engine::core::event_subscription::frames::{
    SSE_KEEPALIVE_EVENT, SSE_KEEPALIVE_INTERVAL,
};
use lucidos_engine::core::event_subscription::ROOT_THREAD_ROLE_ALIASES;
use lucidos_engine::core::preference_catalog::MEMORY_MODULE_WHILE_UNSET;
use lucidos_engine::core::proxy_timeout_bounds::CLIENT_WAIT_SECS;
use lucidos_engine::core::subprocess_origin::{HEADER_DEVICE_ID, QUERY_SOCKET_TICKET};
use lucidos_engine::diagnostic_log::CLIENT_LOG_MAX_BATCH;
use lucidos_engine::engine::archive_policy::MAX_IDS;
use lucidos_engine::engine::archive_policy::{PINNED_SUB_THREAD, SAME_FAMILY};
use lucidos_engine::engine::continuation_reason::TURN_RUNNING_REASON;
use lucidos_engine::engine::continuation_reason::{
    AUTO_RECOVERY_AFTER_HANG_REASON, AUTO_RESUME_AFTER_API_ERROR_REASON,
    AUTO_RESUME_AFTER_SWITCH_REASON, BUILD_REQUESTED_REASON, EFFORT_RESTART_REASON,
    ENGINE_RESTART_INTERRUPT_REASON, HANDOFF_REQUESTED_REASON, HARDEN_REQUESTED_REASON,
    PLAN_REQUESTED_REASON, REPORTS_REQUESTED_REASON, USER_CLICKED_CONTINUE_REASON,
};
use lucidos_engine::engine::permission::Action;
use lucidos_engine::engine::side_question_policy::SIDE_QUESTION_TIMEOUT;
use lucidos_engine::engine::terminal::lifecycle::DISABLED_REASON as TERMINAL_DISABLED_REASON;
use lucidos_engine::engine::terminal::suggestion::BUSY_REASON as TERMINAL_BUSY_REASON;
use lucidos_engine::engine::thread_events::{
    ThreadEvent, KEEP_ALIVE_ENDING_EVENT_TYPES, KEEP_ALIVE_IDLE_REASON,
};
use lucidos_engine::engine::thread_liveness::LivenessState;
use lucidos_engine::engine::widgets::WIDGET_PARAMS_MAX_BYTES;
use lucidos_engine::llm::judgment::endpoint::{
    SystemOneEndpoint, CLOUDFLARE_ACCOUNTS_PREFIX, CLOUDFLARE_WORKERS_AI_CREDENTIAL_SERVICE,
    OPENROUTER_JEV_MODEL, SYSTEM_ONE_CUSTOM_CREDENTIAL_SERVICE, TYPESAFE_API_KEY_ENV,
    TYPESAFE_CREDENTIAL_SERVICE,
};
use lucidos_engine::llm::judgment::WAKE_DECISION_DEFAULT;
use lucidos_engine::scheduler::PresenceOutcome;
use lucidos_engine::voice::sections::SECTIONS;
use lucidos_gateway_constants::DATA_MUTATION_RESPONSE_DEADLINE;

use super::{quoted, REGENERATE};

fn list(values: &[&str]) -> String {
    let quoted: Vec<String> = values.iter().map(|v| quoted(v)).collect();
    format!("[{}] as const", quoted.join(", "))
}

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

    ts.from("../../lucidos-gateway-stamp/src/workspace_client.rs");
    ts.renamed(
        "VERSION",
        "WORKSPACE_CLIENT_VERSION",
        lucidos_gateway_stamp::workspace_client::VERSION,
    );

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
    ts.from("engine/thread_liveness.rs");
    let states = list(&LivenessState::ALL.map(LivenessState::as_str));
    ts.renamed("LivenessState::ALL", "LIVENESS_STATES", states);
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
    ts.from("../../lucidos-gateway-constants/src/lib.rs");
    ts.renamed(
        "DATA_MUTATION_RESPONSE_DEADLINE",
        "DATA_MUTATION_RESPONSE_DEADLINE_MS",
        DATA_MUTATION_RESPONSE_DEADLINE.as_millis(),
    );
    ts.from("core/subprocess_origin.rs");
    ts.export("HEADER_DEVICE_ID", quoted(HEADER_DEVICE_ID));
    ts.export("QUERY_SOCKET_TICKET", quoted(QUERY_SOCKET_TICKET));
    ts.from("engine/terminal/lifecycle.rs");
    ts.renamed(
        "DISABLED_REASON",
        "TERMINAL_DISABLED_REASON",
        quoted(TERMINAL_DISABLED_REASON),
    );
    ts.from("engine/terminal/suggestion.rs");
    ts.renamed(
        "BUSY_REASON",
        "TERMINAL_BUSY_REASON",
        quoted(TERMINAL_BUSY_REASON),
    );
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
    ts.export("GUARD_SHELLS", list(GUARD_SHELLS));
    ts.export("BENIGN_PREFIX_WORDS", list(BENIGN_PREFIX_WORDS));
    ts.export("GRANTS_NOTHING_HEADS", list(grants_nothing_heads()));
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
        ("PLAN_REQUESTED_REASON", PLAN_REQUESTED_REASON),
        ("BUILD_REQUESTED_REASON", BUILD_REQUESTED_REASON),
        ("HANDOFF_REQUESTED_REASON", HANDOFF_REQUESTED_REASON),
        ("REPORTS_REQUESTED_REASON", REPORTS_REQUESTED_REASON),
        ("EFFORT_RESTART_REASON", EFFORT_RESTART_REASON),
    ];
    for (name, value) in reasons {
        ts.export(name, quoted(value));
    }

    ts.from("engine/thread_events/event_impl.rs");
    ts.export("KEEP_ALIVE_IDLE_REASON", quoted(KEEP_ALIVE_IDLE_REASON));
    let ending = list(KEEP_ALIVE_ENDING_EVENT_TYPES);
    ts.export("KEEP_ALIVE_ENDING_EVENT_TYPES", ending);
    ts.renamed(
        "ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES",
        "QUESTION_OVERTAKEN_EVENT_TYPES",
        list(ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES),
    );

    ts.from("../../lucidos-core/src/engine/thread_events/permission/action.rs");
    let steps = serde_json::to_string(&Action::ENGINE_STEPS).expect("engine steps serialize");
    ts.renamed(
        "Action::ENGINE_STEPS",
        "ENGINE_STEP_ACTIONS",
        format!("{steps} as const"),
    );

    ts.from("../../lucidos-core/src/core/event_subscription/mod.rs");
    let aliases =
        serde_json::to_string(&*ROOT_THREAD_ROLE_ALIASES).expect("role event aliases serialize");
    ts.export("ROOT_THREAD_ROLE_ALIASES", format!("{aliases} as const"));

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
    let kinds = list(&summary_entry_kinds());
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
