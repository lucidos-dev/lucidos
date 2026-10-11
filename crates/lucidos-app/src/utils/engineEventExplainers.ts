import {
  CONTINUATION_AUTO_RECOVERY_REASON,
  CONTINUATION_AUTO_RESUME_AFTER_API_ERROR_REASON,
  CONTINUATION_AUTO_RESUME_AFTER_SWITCH_REASON,
  CONTINUATION_HARDEN_REQUESTED_REASON,
  CONTINUATION_USER_CLICKED_REASON,
  plainEventName,
  type AbortCause,
  type CancelCause,
  type EngineReason,
  type EventWaitReason,
} from '../store/thread-events';

/** Why a user-driven `ResponseCanceled` fired, for the route popover.
 *  Mirrors Rust's `CancelCause` doc comments. The heading ("Why the response
 *  stopped") is owned by the renderer; every branch returns a non-empty string
 *  so the row never silently drops. */
export function describeCancelCause(cause: CancelCause | undefined): string {
  switch (cause) {
    case 'user_stop':
      return 'You pressed Cancel while the reply was running.';
    case 'user_action':
      return 'You applied, discarded or archived this thread while the agent was still working, so Lucidos stopped the agent first.';
    case 'superseded_by_followup':
      return 'You sent a follow-up while the agent was working. Lucidos stopped that reply and answered your follow-up next. Work already done is kept.';
    case 'unknown':
    case undefined:
      return 'You stopped the reply. Lucidos did not record how.';
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
      return 'The agent stopped without finishing its reply, or another job had waited a minute for this thread. Lucidos stopped the reply so the thread was not stuck.';
    case 'engine_shutdown':
      return 'Lucidos stopped or restarted while the agent was working.';
    case 'recovery_after_restart':
      // Not "could not be resumed": it can be, and the button to do it is
      // right there. Two emit sites, both a deliberate hold, which is why the
      // text covers both. The boot orphan sweep finds a `running` row with no
      // live process and never learns what killed the last one; the boot floor
      // (`settle_unresumed_switch_threads`) withdraws a switch's resume promise
      // it could not keep. See docs/glossary.md § Cause-gated resume.
      return 'Lucidos restarted and found this reply still marked as running. It did not restart the reply on its own, because that work may be what stopped Lucidos. Continue picks it back up.';
    case 'process_killed':
      return 'The agent’s process ended unexpectedly: it crashed, ran out of memory, or was closed from outside Lucidos.';
    case 'stale_settle':
      return 'This reply was still marked as running, but nothing was working on it. Lucidos cleared the mark. No work was lost.';
    case 'session_dropped':
      return 'Whatever started the agent went away while it was working, most often a closed connection. The agent stopped where it was. Send a message to carry on.';
    case 'unknown':
    case undefined:
      return 'Lucidos stopped the reply. It did not record why.';
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
      return 'You pressed Continue on the stopped reply, and Lucidos picked it back up where it stopped.';
    case CONTINUATION_AUTO_RECOVERY_REASON:
      return 'The agent stopped responding, or a stray signal closed it. Nothing restarted: Lucidos started that one agent again and carried on.';
    case CONTINUATION_AUTO_RESUME_AFTER_SWITCH_REASON:
      return 'You chose Switch on the new version, which stopped this reply. Lucidos picked it back up once the new version was running.';
    case CONTINUATION_HARDEN_REQUESTED_REASON:
      return 'You pressed Harden. Lucidos picked the thread back up to review the work and run its tests, and proposes the change when that finishes.';
    case CONTINUATION_AUTO_RESUME_AFTER_API_ERROR_REASON:
      return 'The connection to the model dropped part-way through the reply above, so it stopped unfinished. Nothing restarted: Lucidos picked the reply back up. It does this a few times in a row at most, then leaves the failure standing.';
    default:
      return null;
  }
}

/** Why the engine acted, in plain words, for the route popover and a boundary
 *  card's Details. The heading ("Why Lucidos acted") is owned by the renderer. */
export function describeEngineReason(reason: EngineReason): string {
  switch (reason.kind) {
    case 'continuation_started':
    case 'session_recovered':
      // Deliberately channel-agnostic: this is the fallback a resume boundary
      // lands on when it recorded no finer `reason`, and `ContinuationStarted`
      // is emitted on chat and trigger threads too, not just coding-agent ones.
      return 'Lucidos stopped while this reply was running, and picked it back up once it was running again.';
    case 'orphan_recovery':
      return 'Lucidos restarted while the agent was working. It kept the work it found and offered it as a change, marked unfinished.';
    case 'harden_retrigger':
      return `${REVIEW_RULE} That check was missing or out of date, so Lucidos asked the agent to run it again.`;
    case 'stale_session':
      return 'The agent behind this thread had stopped running. Lucidos tidied up after it and offered its work as a change.';
    case 'archived_branch_work':
      return 'This thread was archived with work that was never offered as a change. Lucidos set it aside so it is not lost. Bring it back from the Changes panel to apply it.';
    case 'merge_conflict':
      return 'Your changes conflict with newer changes in the project: a merge conflict. Lucidos asked the agent to resolve it before your changes can be applied.';
    case 'missing_hardening':
      return `${REVIEW_RULE} It had not run yet, so Lucidos asked the agent to run it.`;
    case 'missing_plan':
      return 'Changes to Lucidos itself need an approved implementation plan, or a note that the fix is local, before they are offered. This work had neither, so Lucidos held it back and asked the agent for one.';
    case 'plugin_auto_update':
      return `Lucidos found a newer version of ${reason.plugin_id} in ${reason.marketplace_name} and updated the installed plugin.`;
    case 'plugin_setup':
      return describePluginSetup(reason);
    case 'plugin_upstream_proposal':
      return `You asked to offer your local changes to ${reason.plugin_name} to its author. Lucidos saved them as a patch and started this thread so the Lucidos Agent can propose it upstream.`;
    case 'scheduler':
      return reason.trigger_name
        ? `The trigger “${reason.trigger_name}” ran on its schedule and sent this message.`
        : 'A trigger ran on its schedule and sent this message.';
    case 'event_wait':
      return describeEventWait(reason);
  }
}

/** The rule every hardening explainer starts from. */
const REVIEW_RULE = 'Changes must pass hardening, an automatic review and test run, before they are applied.';

/** What a wait watched, in plain words: "a change proposed or a coding agent
 *  stopped working". Empty when the row recorded nothing. */
export function describeWatchedEvents(watched: readonly string[]): string {
  return watched.map(plainEventName).join(' or ');
}

/** The story of a wait re-entry. A legacy row infers only the outcome, so its
 *  `watched` and `wait_reason` are empty, and each clause drops out rather
 *  than printing a blank. */
function describeEventWait({ outcome, watched, wait_reason }: EventWaitReason): string {
  const what = describeWatchedEvents(watched);
  const asked = what
    ? `The agent asked Lucidos to tell it when “${what}” happened`
    : 'The agent asked Lucidos to tell it when an event happened';
  const because = wait_reason ? `, because: ${wait_reason}.` : '.';
  const ending = outcome === 'expired'
    ? ' Nothing happened before its deadline, so Lucidos told the agent, to report back.'
    : ' It happened, so Lucidos told the agent.';
  return `${asked}${because}${ending}`;
}

type PluginSetupReason = Extract<EngineReason, { kind: 'plugin_setup' }>;

function describePluginSetup({ plugin_name: name, version, occasion }: PluginSetupReason): string {
  const handOff = 'Lucidos started this thread so the Lucidos Agent can walk you through it.';
  if (occasion.kind === 'fresh_install') {
    return `You installed ${name}, and its setup needs your input. ${handOff}`;
  }
  const from = occasion.from_version ? ` from ${occasion.from_version}` : '';
  return `You updated ${name}${from} to ${version}, and the new version changed its setup instructions. ${handOff} The agent reuses what you set up before.`;
}

/** The named origin of an engine-seeded thread: a label and a value for the
 *  popover's row, and the same pair as the chip's summary line. Null for a
 *  reason that names no thing of its own, which then shows only "Issued by". */
export function engineReasonHeadline(reason: EngineReason): { label: string; value: string } | null {
  switch (reason.kind) {
    case 'plugin_setup': {
      const { plugin_name: name, version, occasion } = reason;
      if (occasion.kind === 'fresh_install') return { label: 'Plugin install', value: `${name} ${version}` };
      const value = occasion.from_version ? `${name} ${occasion.from_version} → ${version}` : `${name} ${version}`;
      return { label: 'Plugin update', value };
    }
    case 'plugin_upstream_proposal':
      return { label: 'Plugin patch', value: `${reason.plugin_name} ${reason.version}` };
    case 'scheduler':
      return reason.trigger_name ? { label: 'Trigger', value: reason.trigger_name } : null;
    case 'event_wait': {
      const what = describeWatchedEvents(reason.watched);
      return what ? { label: 'Waited for', value: what } : null;
    }
    default:
      return null;
  }
}

/** The device that confirmed the action an engine-seeded thread follows, when
 *  one did. The id only; the caller resolves the name. */
export function engineReasonConfirmingDevice(reason: EngineReason): string | undefined {
  switch (reason.kind) {
    case 'plugin_setup':
    case 'plugin_upstream_proposal':
      return reason.confirmed_on_device_id;
    default:
      return undefined;
  }
}

/** Why the engine acted, for an engine-written message that recorded no
 *  origin at all. Says so rather than inventing a cause. */
export const UNRECORDED_ENGINE_SEED_EXPLAINER =
  'Lucidos wrote this message itself, not you. It did not record why.';
