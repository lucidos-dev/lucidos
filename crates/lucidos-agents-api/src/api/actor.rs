//! Resolve the actor (`MessageOrigin`) for an inbound HTTP request.
//!
//! Every mutating endpoint that emits an event (apply/discard/revert a change,
//! create/edit a thread, run a trigger, etc.) MUST stamp the produced event with
//! an actor so the timeline can show *who* initiated each system action.
//!
//! Cross-workspace caller info travels in the request **body** (the `caller_*`
//! fields on `ChatRequest`), not in headers. Handlers parse and validate those
//! fields up front, then bundle them into a `CallerOrigin` and pass it here.
//!
//! Resolution order (mode = `Human`):
//! 1. **Subprocess origin** (`X-Lucidos-Agent-Origin-Token` header carries a
//!    thread-bound origin token that verifies) → `Api { mode: Agent,
//!    source_thread_id }`, the thread the token was minted for. A
//!    Lucidos-spawned subprocess (CC, run_bash, run_python, scheduled script,
//!    `lucidos` CLI) calling back into the engine is an agent action, never the
//!    user — stamp honestly so the timeline shows "Lucidos Agent" instead of
//!    "You" and the source thread id flows into the route popover.
//! 2. `caller` set → `Workspace` (other Lucidos workspace; carries optional
//!    thread/event id from the request body)
//! 3. `device_id` present     → `Device`, the id alone, when this engine's
//!    starter stamped it (`api::gateway_stamp`). A display resolves the
//!    device's name from it. Unstamped, it is `UnattestedCaller`, carrying the
//!    id as a hint.
//! 4. **Machine-local token** (`X-Lucidos-Local-Token` verifies against the
//!    mode 0600 file the gateway minted) → `UnattestedCaller` in `Engine` mode.
//!    The build-watch and the release scripts reach the engine this way: they
//!    belong to no thread, so they hold no origin token. Any subprocess can
//!    read the token too, so it proves no owner. One switch holds that meaning
//!    (`local_auth::local_process_origin`). See ADR 0169.
//! 5. else                    → `None`. A caller presenting no credential has
//!    said nothing about itself, and there is no honest actor to build. It is
//!    NOT the user: stamping one made dropping a credential buy more than
//!    presenting it. [`require_user_actor`] turns this into a refusal at any
//!    handler that must know who is acting. See ADR 0169.
//!
//! Mode = `Agent` or `Engine`:
//! - With `caller` set → `Workspace` (cross-workspace agent/engine call)
//! - Otherwise → `ThreadLink` when `parent_thread_id` is set, else `None`
//!   (callers must construct `MessageOrigin::Engine { reason }` directly).
//!
//! Caller fields are user-controllable — treat as a display hint only,
//! never for authorization. The subprocess token is process-local secret state
//! (env-injected) and IS authoritative for "did this request come from a
//! Lucidos-spawned process".
//!
//! ## The token is thread-bound, so `source_thread_id` is authenticated
//!
//! It did not used to be. The engine minted one token per startup and handed
//! the same value to every subprocess, then read the thread id off a separate
//! `X-Lucidos-Source-Thread-Id` header that nothing verified, so the token
//! proved "a Lucidos subprocess sent this" and nothing about *which thread*.
//! Any subprocess could claim any source thread id, which made every gate that
//! reads `source_thread_id` (`api::chat::subprocess_chat_legitimate`,
//! `api::internal::coding_agent_diff_refresh`,
//! `engine::agent_session::spawn_gate::spawn_provenance`) an accounting boundary rather
//! than an authorization one.
//!
//! Now each spawn gets its own **thread-bound origin token**, minted by
//! [`mint_agent_origin_token`] and shaped `"<thread>@<depth>@<trigger>.<mac>"`,
//! where the MAC is over the whole prefix under a per-startup secret. A `-`
//! stands in for a thread or a trigger the spawn does not carry.
//! [`subprocess_origin`] recomputes and verifies it, and the **prefix is the
//! source thread id**. There is no second input left to disagree with the
//! token, so there is no claim left to forge: a subprocess can only present the
//! token it was handed, and that token names exactly one thread. The old header
//! is gone.
//!
//! ## The prefix also carries the event-trigger chain depth
//!
//! A script trigger's `lucidos events emit` arrives on an axum request task,
//! which shares nothing with the fire that spawned the script. Without a carry
//! the emit stamps depth 0, restarting the chain, and a script trigger
//! subscribed to the event its own script emits never stops.
//!
//! The depth rides the same signed prefix, so it is authenticated rather than
//! claimed: a caller cannot declare a lower depth to escape the cap. The token
//! value is opaque to every client. The CLI, the Python shim and a coding-agent
//! session all copy the env var into the header without reading it. So widening
//! it needs no change outside this file.
//!
//! A token that does not verify is [`SubprocessOrigin::NotSubprocess`], never a
//! subprocess with weaker attribution. The failure is a downgrade in
//! attribution, never an upgrade in reach.
//!
//! ## The prefix also carries the emitting trigger
//!
//! The third field is `emitting_trigger_id`: which trigger's fire this
//! subprocess *is*. `ACTIVE_TRIGGER_ID` marks a fire's emits, and that
//! task-local stops at a `fork`. So a trigger's script emitting through the
//! `lucidos` CLI could still wake the trigger that ran it. Signing the claim
//! authenticates it the way the source thread id already is. An app through
//! the SDK holds no minted token, so it can state no claim at all.
//!
//! The claim covers the fire, never what the fire hands off. A coding-agent
//! spawn is where the two fields part: it carries the depth and passes `None`
//! for the trigger. ADR 0137 holds that rule and its reasoning, ADR 0138 the
//! depth.

use crate::core::subprocess_origin::{
    subprocess_origin, SubprocessOrigin, HEADER_AGENT_ORIGIN_TOKEN, HEADER_DEVICE_ID,
    HEADER_DEVICE_LABEL,
};
use crate::engine::thread_events::{ActorMode, MessageOrigin, ThreadDirection};
use sqlx::PgPool;
use uuid::Uuid;

pub fn pairing_label_for(headers: &axum::http::HeaderMap, device_id: &str) -> Option<String> {
    if header_str(headers, HEADER_DEVICE_ID).as_deref() != Some(device_id) {
        return None;
    }
    let encoded = header_str(headers, HEADER_DEVICE_LABEL)?;
    let label = urlencoding::decode(&encoded).ok()?;
    let label = label.trim();
    (!label.is_empty()).then(|| label.to_string())
}

/// Cross-workspace origin info, extracted from request body `caller_*` fields.
/// `Some(_)` means this is a cross-workspace POST; `None` means same-workspace.
/// Mutual exclusion vs `parent_thread_id` is enforced upstream by
/// `validate_mode_and_spawn`, so this doesn't need to defend against both.
#[derive(Debug, Clone)]
pub struct CallerOrigin {
    pub workspace: String,
    pub thread_id: Option<Uuid>,
    pub event_id: Option<Uuid>,
    /// Upstream actor mode (Human/Agent/Engine of the calling workspace).
    pub mode: ActorMode,
}

pub fn build_message_origin(
    headers: &axum::http::HeaderMap,
    mode: ActorMode,
    device_id: Option<&str>,
    parent_thread_id: Option<Uuid>,
    parent_thread_title: Option<String>,
    spawning_event_id: Option<Uuid>,
    caller: Option<CallerOrigin>,
) -> Option<MessageOrigin> {
    let user_agent = header_str(headers, "user-agent");
    if let SubprocessOrigin::Subprocess {
        source_thread_id,
        emitting_trigger_id,
        ..
    } = subprocess_origin(headers)
    {
        return Some(MessageOrigin::Api {
            user_agent,
            mode: ActorMode::Agent,
            source_thread_id,
            emitting_trigger_id,
        });
    }
    let local_token = super::local_auth::is_local_process(headers);
    // Another workspace is a claim like a device id: only the gateway's stamp
    // over the caller, its thread and its verdict makes it more.
    if let Some(c) = caller {
        return Some(
            match super::gateway_stamp::workspace_call_verdict(headers, &c.workspace, c.thread_id) {
                Some(verdict) => {
                    let route = super::workspace_operations::routed_origin();
                    MessageOrigin::Workspace {
                        workspace: c.workspace,
                        caller_gateway: route.as_ref().map(|r| r.caller_gateway.clone()),
                        routing_gateway: route.as_ref().map(|r| r.routing_gateway.clone()),
                        thread_id: c.thread_id,
                        event_id: c.event_id,
                        user_agent,
                        mode: c.mode,
                        verdict: Some(verdict),
                        remote_start: route.and_then(|r| r.remote_start),
                    }
                }
                None => MessageOrigin::UnattestedCaller {
                    mode: c.mode,
                    user_agent,
                    claimed_device_id: device_id.map(str::to_string),
                    claimed_local_token: local_token,
                    claimed_workspace: Some(c.workspace),
                    claimed_thread_id: c.thread_id,
                },
            },
        );
    }
    match mode {
        ActorMode::Human => {
            match device_id {
                // Only a stamp makes a device id more than typed text.
                Some(id) if super::gateway_stamp::device_is_stamped(headers, id) => {
                    Some(MessageOrigin::Device {
                        device_id: id.to_string(),
                    })
                }
                Some(id) => Some(MessageOrigin::UnattestedCaller {
                    mode,
                    user_agent,
                    claimed_device_id: Some(id.to_string()),
                    claimed_local_token: local_token,
                    claimed_workspace: None,
                    claimed_thread_id: None,
                }),
                None if local_token => Some(super::local_auth::local_process_origin(user_agent)),
                // Nobody. Not the user: a caller presenting no credential has
                // said nothing about itself, and naming it human is the
                // inversion ADR 0169 removes. `require_user_actor` turns this
                // into a refusal for a handler that must know who is acting.
                None => None,
            }
        }
        ActorMode::Agent | ActorMode::Engine => {
            // No caller → must be same-workspace parent-thread spawn.
            parent_thread_id.map(|id| MessageOrigin::ThreadLink {
                thread_id: id,
                title: parent_thread_title,
                spawning_event_id,
                mode,
                direction: ThreadDirection::Parent,
            })
        }
    }
}

/// The actor for a `User`-initiated mutating endpoint with no parent-thread
/// context (apply/discard/revert, settings writes, etc.).
///
/// `device_id_override` lets a handler that receives the device id in the
/// request body (per-device preferences) supply it; otherwise the
/// `x-lucidos-device-id` header does.
pub fn user_actor(
    headers: &axum::http::HeaderMap,
    device_id_override: Option<&str>,
) -> Option<MessageOrigin> {
    let header_did = header_str(headers, HEADER_DEVICE_ID);
    let effective_did = device_id_override.or(header_did.as_deref());
    build_message_origin(
        headers,
        ActorMode::Human,
        effective_did,
        None,
        None,
        None,
        None,
    )
}

/// What a caller is told when it presented no identity at all.
///
/// It names the four credentials rather than the one this route wanted,
/// because the caller does not know which class the engine put it in. Each is
/// something the right caller already holds, so the message is a route back
/// rather than a wall.
pub const UNIDENTIFIED_CALLER: &str =
    "This request carries no identity, so the engine cannot record who is acting. \
     Present one of four credentials: the thread-bound origin token a Lucidos-spawned \
     subprocess is given, the machine-local token file, an \
     x-lucidos-device-id header for a registered device, or a caller_workspace body \
     field for a call from another workspace.";

const ORIGIN_TOKEN_STALE: &str = "This engine does not recognise this origin token. \
     The engine may have restarted since it was issued. \
     Work that outlives a restart cannot write. Run it again.";

async fn device_is_evidence(pool: &PgPool, device_id: &str) -> bool {
    match crate::core::DeviceStore::is_registered(pool, device_id).await {
        Ok(exists) => exists,
        Err(e) => {
            crate::log_legacy!(
                "[Actor] device lookup for '{}' failed ({}); treating it as registered so a \
                 database blip cannot refuse real work",
                device_id,
                e
            );
            true
        }
    }
}

/// The device id a request claims: `body`, else the `x-lucidos-device-id`
/// header. [`require_user_actor`] reads it, so a per-device route can name the
/// device it writes for.
///
/// Blank-filter EACH source before falling back, never the winner afterwards.
/// A present-but-blank body field would otherwise shadow a good header.
pub(crate) fn claimed_device_id<'a>(
    body: Option<&'a str>,
    headers: &'a axum::http::HeaderMap,
) -> Option<&'a str> {
    body.and_then(blank_filtered)
        .or_else(|| header_device_id(headers))
}

/// [`claimed_device_id`] with the header first. Through the gateway that is
/// the device it authenticated, so a body cannot name another person's device.
/// Every chat send reads it. On a team gateway its device names the speaker
/// (ADR 0649), and a re-paired window's stale body id cannot shadow it.
pub(crate) fn authenticated_device_id<'a>(
    body: Option<&'a str>,
    headers: &'a axum::http::HeaderMap,
) -> Option<&'a str> {
    header_device_id(headers).or_else(|| body.and_then(blank_filtered))
}

fn header_device_id(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(HEADER_DEVICE_ID)
        .and_then(|v| v.to_str().ok())
        .and_then(blank_filtered)
}

fn blank_filtered(s: &str) -> Option<&str> {
    let t = s.trim();
    (!t.is_empty()).then_some(t)
}

/// [`user_actor`], refusing rather than returning nobody.
///
/// The one gate every mutating handler asks. A second spelling of it is how
/// four routes drifted apart before ADR 0169, so callers take this and never
/// re-derive the check.
///
/// **A device id must be stamped or name a registered device.** Attribution
/// accepts any id and a gate cannot. An unstamped id naming nothing is
/// suppressed rather than refused, so the caller falls through to its other
/// credentials. An unstamped id naming a registered device is an
/// `UnattestedCaller`: identified, and never the owner.
///
/// 401 rather than 403: the caller may retry with a credential, and nothing
/// here says its identity would be insufficient. That is a later question, and
/// ADR 0168 clause 4 owns it.
pub(crate) async fn require_user_actor(
    headers: &axum::http::HeaderMap,
    pool: &PgPool,
    device_id_override: Option<&str>,
) -> Result<MessageOrigin, super::error::ApiError> {
    let device_id = match claimed_device_id(device_id_override, headers) {
        Some(id)
            if super::gateway_stamp::device_is_stamped(headers, id)
                || device_is_evidence(pool, id).await =>
        {
            Some(id)
        }
        _ => None,
    };
    build_message_origin(headers, ActorMode::Human, device_id, None, None, None, None).ok_or_else(
        || {
            if headers.contains_key(HEADER_AGENT_ORIGIN_TOKEN) {
                super::error::ApiError::new(
                    axum::http::StatusCode::UNAUTHORIZED,
                    ORIGIN_TOKEN_STALE,
                )
                .with_reason("origin_token_stale")
            } else {
                super::error::ApiError::new(
                    axum::http::StatusCode::UNAUTHORIZED,
                    UNIDENTIFIED_CALLER,
                )
            }
        },
    )
}

/// Why a request was refused at the owner-device gate.
pub(super) enum OwnerGateRefusal {
    AppDocument,
    NotAPerson,
    DeviceUnconfirmed,
}

impl OwnerGateRefusal {
    pub(super) fn render(self, act: &str) -> super::error::ApiError {
        let (reason, message) = match self {
            Self::AppDocument => (
                "owner_gate_app_document",
                format!("This page cannot {act}, even with the owner's device id. \
                         Use the control in the Lucidos UI."),
            ),
            Self::NotAPerson => (
                "owner_gate_not_a_person",
                format!("Only the workspace owner can {act}, using the Lucidos UI. \
                         No agent or script may press that button, on any authority. \
                         If the owner asked you, tell them it is theirs to do and where to find it."),
            ),
            Self::DeviceUnconfirmed => (
                "owner_gate_device_unconfirmed",
                format!("Lucidos could not confirm this device, so it did not {act}. \
                         Try again in a moment."),
            ),
        };
        super::error::ApiError::new(axum::http::StatusCode::FORBIDDEN, message).with_reason(reason)
    }
}

/// [`require_user_actor`], narrowed to the workspace owner at one of their own
/// devices.
///
/// The gate in front of an irreversible, owner-only action (ADR 0192). Only a
/// resolved `Device` passes, and a `Device` implies a gateway stamp. Every
/// other attestation is refused:
///
/// - The **thread-bound origin token**, which proves the opposite of a person:
///   a Lucidos-spawned subprocess. It arrives as `Api { mode: Agent }`.
/// - The **machine-local token**, which names a machine and never a person,
///   whichever way `local_auth`'s switch stands.
/// - A typed device id with no stamp, which arrives as `UnattestedCaller`.
///   The caller is refused.
///
/// A stamped device with no row yet, typically a window the gateway re-paired
/// under a new id, is registered here and admitted: the gateway's stamp is the
/// proof, so no request path needs a client round trip to register first.
///
/// A cross-workspace `caller_workspace` is a body field this route never
/// parses, so such a caller arrives as one of the above or as nobody. This gate
/// also refuses the fail-open probe and an app document borrowing the shell's
/// id.
///
/// `device_id_override` names a device supplied outside the header, as in
/// [`require_user_actor`]. `act` names the action in any refusal this gate
/// renders. Unidentified callers still get the attribution error.
pub(crate) async fn require_owner_device(
    headers: &axum::http::HeaderMap,
    pool: &PgPool,
    event_bus: &crate::engine::event_bus::EventBus,
    device_id_override: Option<&str>,
    act: &str,
) -> Result<MessageOrigin, super::error::ApiError> {
    // An app iframe can borrow the shell's device id from localStorage
    // (ADR 0117, ADR 0144). Apply the same document check as secret reveal.
    if !super::secret_reveal::reveal_request_allowed(
        headers,
        super::secret_reveal::RefererRule::WhenPresent,
    ) {
        crate::log_legacy!(
            "[Actor] Refusing an owner-only action from an app document or device session"
        );
        return Err(OwnerGateRefusal::AppDocument.render(act));
    }
    // Deliberately NOT the `thread_reach` ladder. That gate admits a thread
    // acting outside its own subtree on the owner's standing instruction. Any
    // thread whose newest turn-start event has a `Device` actor carries one.
    // So an agent working inside a turn the user opened would inherit this.
    let actor = require_user_actor(headers, pool, device_id_override).await?;
    if matches!(actor, MessageOrigin::UnattestedCaller { .. }) {
        return Err(super::owner_door::unattested());
    }
    let MessageOrigin::Device { device_id, .. } = &actor else {
        crate::log_legacy!(
            "[Actor] Refusing an owner-only action: the caller is {:?}, not a registered device",
            actor
        );
        return Err(OwnerGateRefusal::NotAPerson.render(act));
    };
    match crate::core::DeviceStore::is_registered(pool, device_id).await {
        Ok(true) => Ok(actor),
        Ok(false) => {
            let registered = crate::core::DeviceStore::register(
                pool,
                event_bus,
                device_id,
                header_str(headers, "user-agent").as_deref(),
                pairing_label_for(headers, device_id).as_deref(),
                Some(actor.clone()),
            )
            .await;
            if let Err(e) = registered {
                crate::log_legacy!(
                    "[Actor] Refusing an owner-only action: device '{}' could not be registered: {}",
                    device_id,
                    e
                );
                return Err(OwnerGateRefusal::DeviceUnconfirmed.render(act));
            }
            Ok(actor)
        }
        Err(_) => {
            crate::log_legacy!(
                "[Actor] Refusing an owner-only action: device '{}' could not be confirmed",
                device_id
            );
            Err(OwnerGateRefusal::DeviceUnconfirmed.render(act))
        }
    }
}

/// What an unattested caller is told at a route only the owner may use.
pub const NOT_ATTESTED_AS_OWNER: &str =
    "Only the workspace owner may do this. Use the Lucidos app, or use a trigger \
     so the job runs as an agent.";

/// Who a caller's credentials say it is, at a door only the owner may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerAttestation {
    /// A device the gateway stamped, the machine-local token while
    /// `local_auth` says it proves the owner.
    Owner,
    /// A Lucidos-spawned subprocess: an agent, a script, or the `lucidos` CLI
    /// one of them runs.
    Agent,
    /// Every other caller, the *unattested caller*.
    Unattested,
}

/// Read a caller's credentials as [`OwnerAttestation`].
pub(crate) fn owner_attestation(headers: &axum::http::HeaderMap) -> OwnerAttestation {
    match user_actor(headers, None) {
        Some(MessageOrigin::Device { .. })
        | Some(MessageOrigin::Api {
            mode: ActorMode::Engine,
            source_thread_id: None,
            emitting_trigger_id: None,
            ..
        }) => OwnerAttestation::Owner,
        Some(MessageOrigin::Api {
            mode: ActorMode::Agent,
            ..
        }) => OwnerAttestation::Agent,
        _ => OwnerAttestation::Unattested,
    }
}

/// Pass only a caller its credentials attest as the workspace owner.
///
/// For a route only a person may use: a human-only setting, a confirmed send,
/// a secret reveal. An agent gets `agent_refusal`, written for the agent
/// reading it. An unattested caller is refused.
///
/// **A layer, not a boundary.** A process running as the owner can still
/// rewrite Lucidos's own files. Only an OS sandbox closes that.
///
/// 403: the caller identified itself, and the answer is still no.
pub(crate) fn require_attested_owner(
    headers: &axum::http::HeaderMap,
    agent_refusal: impl Into<String>,
) -> Result<(), super::error::ApiError> {
    let refusal = match owner_attestation(headers) {
        OwnerAttestation::Owner => return Ok(()),
        OwnerAttestation::Agent => {
            super::error::ApiError::new(axum::http::StatusCode::FORBIDDEN, agent_refusal)
        }
        OwnerAttestation::Unattested => super::owner_door::unattested(),
    };
    crate::log_legacy!(
        "[Actor] Stopped a caller not attested as the owner: {}",
        refusal.message
    );
    Err(refusal)
}

/// [`require_user_actor`], shaped for a handler that returns a bare `Response`.
///
/// The same refusal, rendered rather than propagated. A handler that has not
/// adopted `ApiError` still gates BEFORE it mutates. Leaving the check to its
/// emit would run it after the write had landed.
pub(crate) async fn require_user_actor_response(
    headers: &axum::http::HeaderMap,
    pool: &PgPool,
) -> Result<MessageOrigin, axum::response::Response> {
    use axum::response::IntoResponse as _;
    require_user_actor(headers, pool, None)
        .await
        .map_err(|e| e.into_response())
}

fn header_str(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

#[cfg(test)]
#[path = "actor_tests.rs"]
mod tests;
