use serde::{Deserialize, Serialize};

/// Who semantically drove this request:
/// - `Human`: a person clicked/typed (device, API client, or upstream workspace's human)
/// - `Agent`: an LLM decided (parent thread's agent spawned this, upstream workspace's agent called us)
/// - `Engine`: the runtime/scheduler/recovery decided (no LLM in the loop)
///
/// Orthogonal to `MessageOrigin` (which captures *where* the request entered).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActorMode {
    Human,
    Agent,
    Engine,
}

impl ActorMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
            Self::Engine => "engine",
        }
    }
}

/// Why the engine itself spawned a piece of work without a human or agent prompting it.
/// Surfaced in the route popover so users can see "Engine · auto-resumed after restart".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EngineReason {
    /// Resume after an interrupted response (chat/CC). Past name was
    /// `session_recovered` — kept as serde alias for old DB rows. Renamed
    /// alongside the `ContinuationStarted` event for the same reasons
    /// (session-vs-thread ambiguity; the new response is a continuation, not a
    /// recovery of the prior one).
    #[serde(alias = "session_recovered")]
    ContinuationStarted,
    /// Generic orphan thread recovery (non-CC).
    OrphanRecovery,
    /// Scheduled trigger fired. `trigger_id` is the trigger's user-facing
    /// `config.id` (a string — typically a v4 UUID) so the route popover can
    /// match it directly against `/api/v1/triggers`.
    Scheduler {
        trigger_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trigger_name: Option<String>,
    },
    /// Engine auto-retriggered `/harden` because the marker was stale.
    HardenRetrigger,
    /// Stale Claude Code session detected on startup; changes proposed from its branch.
    StaleSession,
    /// Engine detected a merge conflict pulling main into a CC branch.
    MergeConflict,
    /// Engine detected the harden marker is missing or stale before apply.
    MissingHardening,
    /// Historical: the engine used to silently apply marketplace plugin updates
    /// in the background and stamped the resulting `PluginInstalled` event with
    /// this reason. That behavior was retired — the engine now checks for
    /// updates and notifies the user, who applies them from the Apps section.
    /// Retained (not constructed by new code) so old `PluginInstalled` events
    /// carrying this actor still deserialize.
    PluginAutoUpdate {
        plugin_id: String,
        marketplace_id: String,
        marketplace_name: String,
    },
}

/// Which agent authored a thread event, when a thread carries more than one.
///
/// A thread had exactly one agent until voice put a rented *talker* beside the
/// Lucidos Agent (ADR 0150). So `LucidosAgent` is the `Default`, and a payload
/// naming the origin without naming a participant decodes to it.
///
/// An enum rather than a name string: our own agent is not something a caller
/// could spell two ways, and telling it apart is a `match`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentParticipant {
    /// The workspace's own Lucidos Agent.
    #[default]
    LucidosAgent,
    /// An agent that is not ours, sharing a thread with the Lucidos Agent.
    /// `label` is the speaker name the rendered history prints, so neither
    /// participant reads the other's turn as its own.
    Guest { label: String },
}

impl AgentParticipant {
    /// The name the rendered conversation history calls this participant by.
    ///
    /// Our own agent keeps `Assistant`, unchanged from before a thread could
    /// carry two. Every model reading a transcript already knows that word, and
    /// renaming it would rewrite the history of every existing thread.
    pub fn speaker_label(&self) -> &str {
        match self {
            Self::LucidosAgent => "Assistant",
            Self::Guest { label } => label,
        }
    }
}

/// Direction of a `ThreadLink` origin: which end of the parent⇄child
/// relationship the linked thread sits on relative to the receiving thread.
///
/// - `Parent` — the linked thread *spawned* the receiving thread. Used on
///   the child's first MessageReceived to attribute the spawn.
/// - `Child`  — the linked thread is a *child* of the receiving thread, posting
///   a callback (e.g. `[Child thread completed] ...`) back into the parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadDirection {
    Parent,
    Child,
}

fn default_thread_direction_parent() -> ThreadDirection {
    // Historical DB rows only ever carried the parent direction (the variant
    // used to be named ParentThread). New writes always set it explicitly.
    ThreadDirection::Parent
}

/// Who this thread event is the work of.
///
/// Usually that is where an inbound `MessageReceived` entered, stamped at the
/// HTTP boundary in `api/chat.rs::chat_submit`. `Agent` is the outbound case:
/// an agent in the thread authored the event, and the variant names which one
/// (ADR 0150).
///
/// Optional on the wire so old DB rows deserialize cleanly. The frontend has a
/// `legacyOrigin()` fallback that synthesizes Device / ThreadLink from the
/// legacy `device_id` / `parent_thread_id` fields when origin is missing.
///
/// Invariants enforced at construction in `engine/chat/events.rs`:
/// - `Device { .. }`            ⇒ `mode == ActorMode::Human` (intrinsic)
/// - `Api { mode, .. }`         ⇒ `mode == carried mode` (Human, Agent, or Engine)
/// - `Workspace { mode, .. }`   ⇒ `mode == carried mode`
/// - `ThreadLink { mode, .. }`  ⇒ `mode == carried mode`
/// - `Engine { .. }`            ⇒ `mode == ActorMode::Engine` (intrinsic)
///
/// Workspace caller_* body fields are display hints only — they are user-controllable
/// (any HTTP client can send them) and MUST NOT be used for authorization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MessageOrigin {
    /// Mode = Human. Human at a known device. `device_id` is the TEXT primary
    /// key from the `devices` table (not necessarily a UUID).
    Device { device_id: String, label: String },
    /// HTTP request without a `device_id` and without a `caller_workspace`
    /// body field. Mode is carried explicitly so SDK callers can declare
    /// themselves as agent/engine; defaults to Human for back-compat.
    ///
    /// `source_thread_id` is populated when the request arrived from a
    /// Lucidos-spawned subprocess (CC, `run_bash`, `run_python`, scheduled
    /// script, `lucidos` CLI) — detected via the per-engine-startup token
    /// (`HEADER_AGENT_ORIGIN_TOKEN`). When set, `mode` is `Agent` and the
    /// frontend route popover links back to the spawning thread.
    Api {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_agent: Option<String>,
        /// Defaults to Human because all pre-existing API calls were human-initiated;
        /// agent/engine-initiated API calls must set this explicitly.
        #[serde(default = "default_api_mode")]
        mode: ActorMode,
        /// Spawning thread of the subprocess that issued this request, when
        /// the engine recognised the subprocess origin token. `None` for
        /// external API clients (browsers, real curl, cross-workspace
        /// engines) and for legacy persisted rows that pre-date the field.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_thread_id: Option<uuid::Uuid>,
    },
    /// Mode = Human or Agent (carried in `mode`). HTTP request from another
    /// Lucidos workspace, identified by the `caller_workspace` body field.
    Workspace {
        workspace: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<uuid::Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<uuid::Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_agent: Option<String>,
        /// Upstream intent. Defaults to Human because all pre-existing
        /// cross-workspace calls were human-initiated; agent/engine-initiated
        /// cross-workspace calls must set this explicitly via the body's `mode`
        /// field. Read by `MessageOrigin::mode()` to derive the request's `ActorMode`.
        #[serde(default = "default_workspace_mode")]
        mode: ActorMode,
    },
    /// Mode = Human, Agent, or Engine (carried in `mode`). Bidirectional link
    /// to another thread in the same workspace. `direction = Parent` means
    /// the linked thread *spawned* the receiving thread; `direction = Child`
    /// means the linked thread is a child posting a callback back into the
    /// receiving (parent) thread.
    ///
    /// `serde(alias = "parent_thread")` keeps historical DB rows readable —
    /// they all originate from the unidirectional `ParentThread` variant, so
    /// `direction` defaults to `Parent` when missing.
    #[serde(alias = "parent_thread")]
    ThreadLink {
        thread_id: uuid::Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawning_event_id: Option<uuid::Uuid>,
        /// Defaults to Agent because all pre-existing thread links were
        /// LLM-driven; engine-initiated links must set this explicitly.
        /// Read by `MessageOrigin::mode()` to derive the request's `ActorMode`.
        #[serde(default = "default_thread_link_mode")]
        mode: ActorMode,
        #[serde(default = "default_thread_direction_parent")]
        direction: ThreadDirection,
    },
    /// Mode = Agent. An agent participating in this thread authored the event,
    /// and `agent` names which one (ADR 0150). The outbound counterpart of the
    /// variants above, which all name a way IN.
    ///
    /// Not stamped on a user action that merely ends an agent's turn: a
    /// `ResponseCanceled` carries the person who clicked Stop.
    Agent {
        #[serde(default)]
        agent: AgentParticipant,
    },
    /// An inbound webhook fired. `name` is what the user called it, so the
    /// timeline says which hook rather than "API caller".
    ///
    /// Mode is Engine: a webhook is neither a person nor an agent, and Engine
    /// is the existing deterministic, non-human mode. The caller is external
    /// and unauthenticated until the engine verifies it, so nothing here is
    /// ever an authorization input.
    Webhook { webhook_id: String, name: String },
    /// The engine itself initiated this work (recovery, scheduler, retrigger).
    /// Implies `ActorMode::Engine`.
    Engine { reason: EngineReason },
    /// The host system killed the underlying process (engine shutdown, OS signal,
    /// crash, safety net catch). The engine only marks the abort — it didn't
    /// decide to abort. Distinct from `Engine` which represents engine-deliberate
    /// actions (hardening retrigger, merge conflict, scheduler). Renders as
    /// "System" in the UI. Implies `ActorMode::Engine` (deterministic, non-human,
    /// non-agent), but the chip label differentiates from engine-deliberate work.
    System,
}

fn default_workspace_mode() -> ActorMode {
    ActorMode::Human
}

fn default_thread_link_mode() -> ActorMode {
    ActorMode::Agent
}

fn default_api_mode() -> ActorMode {
    ActorMode::Human
}

impl MessageOrigin {
    /// Derive the actor mode from the origin variant. For `Workspace` and
    /// `ThreadLink` this reads the carried field; for the rest the mode is
    /// intrinsic to the variant.
    pub fn mode(&self) -> ActorMode {
        match self {
            Self::Device { .. } => ActorMode::Human,
            Self::Api { mode, .. } => *mode,
            Self::Workspace { mode, .. } | Self::ThreadLink { mode, .. } => *mode,
            Self::Agent { .. } => ActorMode::Agent,
            Self::Webhook { .. } | Self::Engine { .. } | Self::System => ActorMode::Engine,
        }
    }

    /// The agent that authored this event, when an agent did.
    pub fn agent(&self) -> Option<&AgentParticipant> {
        match self {
            Self::Agent { agent } => Some(agent),
            _ => None,
        }
    }

    /// Convenience constructor for engine-initiated origins. Tightens emit sites
    /// like `Some(MessageOrigin::engine(EngineReason::ContinuationStarted))`.
    pub fn engine(reason: EngineReason) -> Self {
        Self::Engine { reason }
    }

    /// Convenience constructor for system-killed-process aborts.
    /// Use for `ResponseAborted` paths where the underlying process died
    /// (engine shutdown, safety-net catch, recovery after restart, OS signal).
    /// NOT for engine-deliberate actions like hardening retrigger or scheduler.
    pub fn system() -> Self {
        Self::System
    }

    /// Convenience constructor for child→parent callback origins. The
    /// receiving thread is the parent; `child_thread_id` is the child whose
    /// completion produced the message. Title/spawning_event_id default to
    /// `None` since the parent already has its own thread context.
    pub fn thread_link_child(child_thread_id: uuid::Uuid, mode: ActorMode) -> Self {
        Self::ThreadLink {
            thread_id: child_thread_id,
            title: None,
            spawning_event_id: None,
            mode,
            direction: ThreadDirection::Child,
        }
    }
}
