import {
  CONTINUATION_AUTO_RECOVERY_REASON,
  CONTINUATION_AUTO_RESUME_AFTER_API_ERROR_REASON,
  CONTINUATION_AUTO_RESUME_AFTER_SWITCH_REASON,
  CONTINUATION_BUILD_REQUESTED_REASON,
  CONTINUATION_EFFORT_RESTART_REASON,
  CONTINUATION_HANDOFF_REQUESTED_REASON,
  CONTINUATION_HARDEN_REQUESTED_REASON,
  CONTINUATION_PLAN_REQUESTED_REASON,
  CONTINUATION_REPORTS_REQUESTED_REASON,
  CONTINUATION_USER_CLICKED_REASON,
  plainEventName,
  type AbortCause,
  type CancelCause,
  type EngineReason,
  type EventWaitReason,
  type PluginSetupCause,
  type PluginSetupOccasion,
} from '../store/thread-events';

export function describeCancelCause(cause: CancelCause | undefined): string {
  switch (cause) {
    case 'user_stop':
      return 'You pressed Cancel while the reply was running.';
    case 'user_action':
      return 'You applied, discarded or archived this thread while the agent was still working, so Lucidos stopped the agent first.';
    case 'superseded_by_followup':
      return 'You sent a follow-up while the agent was working. Lucidos stopped that reply and answered your follow-up next. Work already done is kept.';
    case 'effort_restart':
      return 'You picked a new reasoning effort. This agent takes effort only when it starts, so Lucidos stopped this reply and restarted the agent at the new effort. Work already done is kept.';
    case 'refused':
      return "You chose Don't allow on the card, so the coding agent never started.";
    case 'hold':
      return 'This thread was put on hold, so Lucidos stopped the reply. Nothing starts here by itself until Resume.';
    case 'unknown':
    case undefined:
      return 'You stopped the reply. Lucidos did not record how.';
  }
}

export function describeAbortCause(cause: AbortCause | undefined): string {
  switch (cause) {
    case 'safety_net':
      return 'The agent stopped without finishing its reply, or another job had waited a minute for this thread. Lucidos stopped the reply so the thread was not stuck.';
    case 'engine_shutdown':
      return 'Lucidos stopped or restarted while the agent was working.';
    case 'recovery_after_restart':
      return 'Lucidos restarted and found this reply still marked as running. It did not restart the reply on its own, because that work may be what stopped Lucidos. Continue picks it back up.';
    case 'process_killed':
      return 'The agent’s process ended unexpectedly: it crashed, ran out of memory, or was closed from outside Lucidos.';
    case 'stale_settle':
      return 'This reply was still marked as running, but nothing was working on it. Lucidos cleared the mark. No work was lost.';
    case 'snapshot_restored':
      return 'The workspace was restored from a snapshot that recorded this reply as running. Lucidos stopped the reply and did not replay it.';
    case 'session_dropped':
      return 'Whatever started the agent went away while it was working, most often a closed connection. The agent stopped where it was. Send a message to carry on.';
    case 'unknown':
    case undefined:
      return 'Lucidos stopped the reply. It did not record why.';
  }
}

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
    case CONTINUATION_PLAN_REQUESTED_REASON:
      return 'Automatic steps on this thread were paused, and you let one run: Lucidos sent the agent back to settle its plan marker.';
    case CONTINUATION_BUILD_REQUESTED_REASON:
      return 'Automatic steps on this thread were paused, and you let one run: Lucidos told the agent to start building the plan you approved.';
    case CONTINUATION_HANDOFF_REQUESTED_REASON:
      return 'Automatic steps on this thread were paused, and you let one run: Lucidos asked the agent to write its handoff, since its session passed its context ceiling.';
    case CONTINUATION_REPORTS_REQUESTED_REASON:
      return 'Automatic steps on this thread were paused, and you let one run: Lucidos woke the agent to read the reports that arrived meanwhile.';
    case CONTINUATION_EFFORT_RESTART_REASON:
      return 'You picked a new reasoning effort. This agent takes effort only when it starts, so Lucidos restarted it at the new effort and it carried on where it stopped.';
    case CONTINUATION_AUTO_RESUME_AFTER_API_ERROR_REASON:
      return 'The connection to the model dropped part-way through the reply above, so it stopped unfinished. Nothing restarted: Lucidos picked the reply back up. It does this a few times in a row at most, then leaves the failure standing.';
    default:
      return null;
  }
}

export const CACHE_KEEP_ALIVE_EXPLAINER = 'The agent was waiting, so Lucidos pinged the model to keep its prompt cache warm. Once that cache expires, after 5 minutes or an hour depending on how the agent is billed, its next step pays to read the whole conversation again.';

export function describeEngineReason(reason: EngineReason): string {
  switch (reason.kind) {
    case 'continuation_started':
    case 'session_recovered':
      return 'Lucidos stopped while this reply was running, and picked it back up once it was running again.';
    case 'orphan_recovery':
      return 'Lucidos restarted while the agent was working. It kept the work it found and offered it as a change, marked unfinished.';
    case 'harden_retrigger':
      return `${REVIEW_RULE} That check was missing or out of date, so Lucidos asked the agent to run it again.`;
    case 'stale_session':
      return 'The agent behind this thread had stopped running. Lucidos tidied up after it and offered its work as a change.';
    case 'archived_branch_work':
      return 'This thread was archived with work that was never offered as a change. Lucidos kept it on the thread’s branch so it is not lost. Unarchive the thread to offer it again as a change.';
    case 'unmerged_integrated_work':
      return 'This thread’s work was meant to reach the project inside its parent’s change. That change was applied without it. Lucidos kept the work on the thread’s branch. Resume the thread, or unarchive it, to offer the work again as a change.';
    case 'successor_residue':
      return 'This thread kept working after it continued in a legacy successor thread. You applied or discarded the successor’s change without that later work, so Lucidos kept the work on the thread’s branch. Resume the thread, or unarchive it, to offer the work again as a change.';
    case 'successor_session':
      return 'The previous session in this thread handed off, so Lucidos started a fresh one to carry on with a clean context. Its first message is the handoff.';
    case 'merge_conflict':
      return 'Your changes conflict with newer changes in the project: a merge conflict. Lucidos asked the agent to resolve it before your changes can be applied.';
    case 'main_check_red':
      return 'The main check failed: the tests that run on GitHub after changes land found main broken. Lucidos started this thread to find the cause and fix it.';
    case 'main_check_flaky':
      return 'The main check passed, but some tests failed once and passed on a retry. Lucidos started this thread to find what makes them flaky and fix it.';
    case 'missing_hardening':
      return `${REVIEW_RULE} It had not run yet, so Lucidos asked the agent to run it.`;
    case 'missing_plan':
      return 'Changes to Lucidos itself need an approved implementation plan, or a note that the fix is local, before they are offered. Lucidos held this work back at the plan check and told the agent how to clear it.';
    case 'approved_plan_unstarted':
      return 'You approved building the plan here, but the turn ended before any code changed. Lucidos told the agent to start the first phase.';
    case 'model_substituted':
      return 'Claude Code ran another model than this thread asked for, because the account could not run it. Lucidos asked the agent to check with you which rung to use instead, and whether to take the model off the ladder.';
    case 'context_ceiling':
      return 'The agent’s context grew past this thread’s context ceiling. Lucidos asked it to write down where the work stands, so a fresh start can carry on from there with a clean context.';
    case 'plugin_auto_update':
      return `Lucidos found a newer version of ${reason.plugin_id} in ${reason.marketplace_name} and updated the installed plugin.`;
    case 'plugin_setup':
      return describePluginSetup(reason);
    case 'plugin_upstream_proposal':
      return `You asked to offer your local changes to ${reason.plugin_name} to its author. Lucidos saved them as a patch and started this thread so the Lucidos Agent can propose it upstream.`;
    case 'scheduler': {
      const ran = reason.trigger_name
        ? `The trigger “${reason.trigger_name}” ran on its schedule and sent this message.`
        : 'A trigger ran on its schedule and sent this message.';
      return reason.actor_email ? `${ran} It carries an action by ${reason.actor_email}.` : ran;
    }
    case 'event_wait':
      return describeEventWait(reason);
    case 'cache_keep_alive':
      return CACHE_KEEP_ALIVE_EXPLAINER;
  }
}

const REVIEW_RULE = 'Changes must pass hardening, an automatic review and test run, before they are applied.';

export function describeWatchedEvents(watched: readonly string[]): string {
  return watched.map(plainEventName).join(' or ');
}

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
  const cause = occasion.cause ?? { kind: 'instructions_changed' };
  return `You updated ${name}${from} to ${version}, and ${describeSetupCause(cause)}${authorNote(cause)} ${handOff} The agent reuses what you set up before.`;
}

function describeSetupCause(cause: PluginSetupCause): string {
  switch (cause.kind) {
    case 'instructions_changed':
      return 'the new version changed its setup instructions.';
    case 'revision_raised':
      return 'its author asks to run its setup again.';
    case 'both':
      return 'the new version changed its setup instructions, and its author asks to run it again.';
  }
}

function authorNote(cause: PluginSetupCause): string {
  return 'reason' in cause && cause.reason ? ` Author's note: ${cause.reason}` : '';
}

export function describeSetupRunNote(run: PluginSetupOccasion | null | undefined): string {
  if (run?.kind !== 'update') return 'Setup does not run again: nothing in it changed.';
  const cause = run.cause ?? { kind: 'instructions_changed' };
  const why = {
    instructions_changed: 'the steps changed.',
    revision_raised: 'the author asks for it.',
    both: 'the steps changed and the author asks for it.',
  }[cause.kind];
  return `Setup runs again after this update: ${why}${authorNote(cause)}`;
}

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

export function engineReasonConfirmingDevice(reason: EngineReason): string | undefined {
  switch (reason.kind) {
    case 'plugin_setup':
    case 'plugin_upstream_proposal':
      return reason.confirmed_on_device_id;
    default:
      return undefined;
  }
}

export const UNRECORDED_ENGINE_SEED_EXPLAINER =
  'Lucidos wrote this message itself, not you. It did not record why.';
