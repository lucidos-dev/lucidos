import {
  CONTINUATION_AUTO_RECOVERY_REASON,
  CONTINUATION_AUTO_RESUME_AFTER_API_ERROR_REASON,
  CONTINUATION_AUTO_RESUME_AFTER_SWITCH_REASON,
  CONTINUATION_USER_CLICKED_REASON,
  type AbortCause,
  type CancelCause,
  type EngineReason,
} from '../store/thread-events';

/** Why a user-driven `ResponseCanceled` fired, for the Initiator info popover.
 *  Mirrors Rust's `CancelCause` doc comments. The heading ("Why the response
 *  stopped") is owned by the renderer; every branch returns a non-empty string
 *  so the row never silently drops. */
export function describeCancelCause(cause: CancelCause | undefined): string {
  switch (cause) {
    case 'user_stop':
      return 'You clicked Cancel on the running response.';
    case 'user_action':
      return 'You applied, discarded, or archived a still-running Claude Code session — the action stopped the current turn first.';
    case 'superseded_by_followup':
      return 'You posted a follow-up while Codex was mid-turn — the engine interrupted that turn and ran your follow-up next, preserving partial work.';
    case 'unknown':
    case undefined:
      return 'You canceled the response (cause not recorded).';
  }
}

/** Why a system-driven `ResponseAborted` fired, for the route popover. The
 *  heading ("Why the response stopped") is owned by the renderer. Each branch
 *  must return a non-empty string — the renderer suppresses the explainer row
 *  on null/undefined, so silently dropping a cause would regress the panel
 *  back to the "Unknown" wart this helper exists to fix. */
export function describeAbortCause(cause: AbortCause | undefined): string {
  switch (cause) {
    case 'safety_net':
      return 'The Claude Code event loop ended without a clean response — usually a Claude Code session crash or driver task death. (The 10-minute hung-session watchdog used to land here too, but now auto-resumes the session via ContinuationRequested instead.)';
    case 'engine_shutdown':
      return 'The engine shut down (or restarted) while this turn was in flight.';
    case 'recovery_after_restart':
      return 'The engine recovered the thread after a restart; the in-flight turn could not be resumed.';
    case 'process_killed':
      return 'The Claude Code session was killed (crash, signal, or out-of-memory).';
    case 'stale_settle':
      return 'The engine cleaned up a stuck response state — no live work was running.';
    case 'session_dropped':
      return 'Whatever started this session was cancelled while it was still running, so the session went down with it — most often a request whose client disconnected. The work stopped where it was; start it again to continue.';
    case 'unknown':
    case undefined:
      return 'The response was interrupted by the system (cause not recorded).';
  }
}

/** Why a `ContinuationStarted` resume boundary exists, for the route popover.
 *  The heading ("Why this resumed") is owned by the renderer.
 *
 *  Keyed on the event's own `reason` rather than on the coarser `EngineReason`,
 *  because the distinction is load-bearing: an `auto_recovery_after_hang`
 *  resume is a LOCAL interruption and must not claim an engine restart, the
 *  same honesty rule `continuationStartedSummary` enforces for the turn header.
 *
 *  Returns null for a legacy row that recorded no reason, so the popover shows
 *  the "Issued by" row alone rather than inventing a plausible cause. */
export function describeContinuationReason(reason: string | undefined): string | null {
  switch (reason) {
    case CONTINUATION_USER_CLICKED_REASON:
      return 'You clicked Continue on the interrupted response, and the engine picked the session back up where it stopped.';
    case CONTINUATION_AUTO_RECOVERY_REASON:
      return 'The agent session stopped responding, or a stray signal killed it. Nothing restarted: the engine relaunched that one session and carried on.';
    case CONTINUATION_AUTO_RESUME_AFTER_SWITCH_REASON:
      return 'You chose Switch to new version, which stopped this response mid-flight. The engine resumed it automatically once the new version was up.';
    case CONTINUATION_AUTO_RESUME_AFTER_API_ERROR_REASON:
      return 'The connection to the model dropped part-way through the response above, so that turn failed incomplete. Nothing restarted: the engine picked the session back up and carried on. It will do this a few times in a row at most, then leave the failure standing.';
    default:
      return null;
  }
}

/** Why the engine acted, for the route popover. Returns null for `scheduler`
 *  because that variant has its own richer renderer (links to the trigger).
 *  The popover heading ("Why the engine acted") is owned by the renderer. */
export function describeEngineReason(reason: EngineReason): string | null {
  switch (reason.kind) {
    case 'continuation_started':
    case 'session_recovered':
      // Deliberately channel-agnostic: this is the fallback a resume boundary
      // lands on when it recorded no finer `reason`, and `ContinuationStarted`
      // is emitted on chat and trigger threads too, not just coding-agent ones.
      return 'A response still running when the engine stopped is auto-resumed once the engine is back. This event marks the resume.';
    case 'orphan_recovery':
      return 'After a restart, the engine resumes orphaned threads where work was in flight.';
    case 'harden_retrigger':
      return 'The engine re-triggers `/harden` when the hardening marker is missing or stale, so changes aren’t applied unhardened.';
    case 'stale_session':
      return 'The engine cleans up Claude Code sessions that became stale (process gone, marker missing). This event marks the cleanup.';
    case 'merge_conflict':
      return 'The engine detected a conflict when merging changes from main into your branch. We need to resolve it before applying.';
    case 'missing_hardening':
      return 'Hardening (`/harden`) must run before changes are applied. The engine queues it automatically when the marker is missing.';
    case 'plugin_auto_update':
      return `The engine found a newer version of ${reason.plugin_id} in ${reason.marketplace_name} and updated the installed plugin.`;
    case 'scheduler':
      return null;
  }
}
