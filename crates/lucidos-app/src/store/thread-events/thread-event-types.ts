import type {
  AbortCause,
  ActorMode,
  BackgroundWaitTiming,
  CancelCause,
  EventSubscription,
  MessageOrigin,
  ThreadEvent,
} from '../../generated/thread-event-wire';
import { ENGINE_STEP_ACTIONS } from '@lucidos/engine-constants';

export type {
  AbortCause,
  ActorMode,
  AgentParticipant,
  AllowScope,
  AnswerKind,
  CancelCause,
  ChildCompletionStatus,
  CodingAgentExit,
  CodingAgentExitStage,
  EngineReason,
  EventChannel,
  EventSubscription,
  EventWaitCancelCause,
  FormRequestOutcome,
  FireRetry,
  MessageOrigin,
  PluginSetupCause,
  PluginSetupOccasion,
  QuestionOption,
  RetryOf,
  RetrySkipReason,
  SessionEndReason,
  SpawnVerdict,
  SubThreadPendingChange,
  ThreadDirection,
  ThreadEvent,
  TodoItem,
  TodoStatus,
  TransientEvent,
  TriggerInvocation,
  VoiceSessionEndReason,
} from '../../generated/thread-event-wire';
import {
  AUTO_RECOVERY_AFTER_HANG_REASON,
  AUTO_RESUME_AFTER_API_ERROR_REASON,
  AUTO_RESUME_AFTER_SWITCH_REASON,
  BUILD_REQUESTED_REASON,
  EFFORT_RESTART_REASON,
  ENGINE_RESTART_INTERRUPT_REASON,
  HANDOFF_REQUESTED_REASON,
  HARDEN_REQUESTED_REASON,
  PLAN_REQUESTED_REASON,
  REPORTS_REQUESTED_REASON,
  USER_CLICKED_CONTINUE_REASON,
} from '@lucidos/engine-constants';
export { THREAD_EVENT_TYPE_NAMES } from '../../generated/thread-event-wire';

export function isTriggerGrantSelection(event: Extract<ThreadEvent, { type: 'UserQuestionAsked' }>): boolean {
  return event.permission_request?.batch?.items.some(item =>
    item.kind === 'reviewed_trigger_program_grants' && item.selectable === true,
  ) ?? false;
}

export function isEngineRaisedCard(event: Extract<ThreadEvent, { type: 'UserQuestionAsked' }>): boolean {
  return event.permission_request !== undefined && event.permission_request.prepared === undefined;
}

export function wakesNoAgent(event: Extract<ThreadEvent, { type: 'UserQuestionAsked' }>): boolean {
  return isEngineRaisedCard(event) && !engineSteps.has(event.permission_request?.action ?? '');
}

const engineSteps: ReadonlySet<string> = new Set(ENGINE_STEP_ACTIONS);

export const FORM_REQUEST_TYPES = [
  'CredentialRequested',
  'PluginInstallRequested',
  'PluginUninstallRequested',
  'EmailConfirmRequested',
  'OAuthAuthorizationRequested',
] as const;

export type FormRequestEvent = Extract<
  ThreadEvent,
  { type: (typeof FORM_REQUEST_TYPES)[number] }
>;

export function isFormRequest(event: { type: string }): event is FormRequestEvent {
  return (FORM_REQUEST_TYPES as readonly string[]).includes(event.type);
}

export const SIDE_QUESTION_TYPES = [
  'SideQuestionAsked',
  'SideQuestionAnswered',
  'SideQuestionFailed',
  'SideQuestionDismissed',
] as const;

export type SideQuestionEvent = Extract<
  ThreadEvent,
  { type: (typeof SIDE_QUESTION_TYPES)[number] }
>;

export function isSideQuestionEvent(event: { type: string }): event is SideQuestionEvent {
  return (SIDE_QUESTION_TYPES as readonly string[]).includes(event.type);
}

export const LEGACY_WIDGET_PIN_TYPES = ['WidgetPinned', 'WidgetUnpinned', 'WidgetRestored', 'WidgetHidden'] as const;

export type ThreadInitiator = 'user' | 'system';

export const ENGINE_LABEL = 'Lucidos Engine';

export const SYSTEM_LABEL = 'System';

export const LUCIDOS_AGENT_LABEL = 'Lucidos Agent';

export const API_CALLER_LABEL = 'API caller';

export function unattestedCallerLabel(
  origin: Extract<MessageOrigin, { kind: 'unattested_caller' }>,
  deviceName?: string,
): string {
  if (origin.claimed_workspace) return `Workspace "${origin.claimed_workspace}"`;
  if (origin.claimed_device_id) return deviceName
    ? `A program that named your device "${deviceName}"`
    : 'A program that named a device';
  return origin.claimed_local_token ? 'A program on this computer' : 'An unknown caller';
}

export function originMode(origin: MessageOrigin | undefined): ActorMode {
  if (!origin) return 'engine';
  switch (origin.kind) {
    case 'device':      return 'human';
    case 'saved_draft': return 'human';
    case 'api':         return origin.mode ?? 'human';
    case 'unattested_caller': return origin.mode ?? 'human';
    case 'workspace':   return origin.mode ?? 'human';
    case 'thread_link': return origin.mode ?? 'agent';
    case 'agent':       return 'agent';
    case 'webhook':     return 'engine';
    case 'event_hook':  return 'engine';
    case 'engine':      return 'engine';
    case 'system':      return 'engine';
  }
}

export function isEngineDownAbort(cause: AbortCause | undefined): boolean {
  return cause === 'engine_shutdown';
}

export function isSwitchTeardownAbort(
  actor: MessageOrigin | undefined,
  cause: AbortCause | undefined,
): boolean {
  return isEngineDownAbort(cause) && actor?.kind === 'device';
}

export function responseAbortedSummary(
  actor: MessageOrigin | undefined,
  cause: AbortCause | undefined,
): string {
  if (cause === 'stale_settle') return 'Stuck reply cleared';
  return isSwitchTeardownAbort(actor, cause) ? 'Paused by restart' : 'Response interrupted';
}

const ABORT_CAUSE_STATE: Record<AbortCause, string> = {
  engine_shutdown: 'Lucidos stopped',
  safety_net: 'Looked stuck',
  recovery_after_restart: 'After restart',
  process_killed: 'Agent stopped unexpectedly',
  stale_settle: 'Nothing was running',
  session_dropped: 'Its caller went away',
  snapshot_restored: 'The workspace was restored from a snapshot',
  unknown: 'Reason not recorded',
};

export function responseAbortedState(
  actor: MessageOrigin | undefined,
  cause: AbortCause | undefined,
): string {
  if (isSwitchTeardownAbort(actor, cause)) return 'Paused by restart';
  return ABORT_CAUSE_STATE[cause ?? 'unknown'];
}

export function isUserStoppedWait(event: { type: string; cause?: string }): boolean {
  return event.type === 'EventWaitCanceled' && event.cause === 'user_stop';
}

export function isTurnlessBoundary(event: { type: string; cause?: string }): boolean {
  return isUserStoppedWait(event)
    || event.type === 'ChildThreadStopped'
    || event.type === 'ChildThreadDetached'
    || event.type === 'ChildReportFiled'
    || event.type === 'CacheKeepAliveStatusChanged'
    || event.type === 'UserOnlyMessagePosted';
}

export function awaitedSubject(reason: string): string {
  const stripped = reason.replace(/^\s*wait(?:ing)?\s+(?:for|on|until)\s+/i, '');
  return stripped.trim() ? stripped : reason;
}

const LEADING_GERUND =
  /^\s*(?!(?:(?:some|any|every|no)?thing|morning|evening|spring|string|pending|staging|incoming|upcoming|remaining|outstanding)\b)[a-z]+ing\b/i;

const LEADING_DETERMINER = /^(?:The|A|An|This|That|These|Those|All|Every|Each|Some|Any|My|Your|Its)\b/;

export function waitingFor(verb: string, reason: string): string {
  const subject = awaitedSubject(reason);
  if (LEADING_GERUND.test(subject)) return `${verb}: ${subject}`;
  const lowered = LEADING_DETERMINER.test(subject)
    ? subject[0].toLowerCase() + subject.slice(1)
    : subject;
  return `${verb} for ${lowered}`;
}

export function eventWaitStoppedSummary(reason: string | undefined): string {
  return reason ? waitingFor('Stopped waiting', reason) : 'Stopped waiting for an event';
}

export const RESPONSE_CANCELED_SUMMARY = 'Response canceled';

const REDIRECT_CANCEL: Record<CancelCause, boolean> = {
  user_stop: false,
  user_action: false,
  superseded_by_followup: true,
  effort_restart: true,
  refused: false,
  hold: false,
  unknown: false,
};

export function isRedirectCancel(cause: CancelCause | undefined): boolean {
  return cause !== undefined && REDIRECT_CANCEL[cause];
}

export const CONTINUATION_AUTO_RECOVERY_REASON = AUTO_RECOVERY_AFTER_HANG_REASON;

export const CONTINUATION_USER_CLICKED_REASON = USER_CLICKED_CONTINUE_REASON;

export const CONTINUATION_AUTO_RESUME_AFTER_SWITCH_REASON = AUTO_RESUME_AFTER_SWITCH_REASON;

export const CONTINUATION_HARDEN_REQUESTED_REASON = HARDEN_REQUESTED_REASON;

export const CONTINUATION_PLAN_REQUESTED_REASON = PLAN_REQUESTED_REASON;
export const CONTINUATION_BUILD_REQUESTED_REASON = BUILD_REQUESTED_REASON;
export const CONTINUATION_HANDOFF_REQUESTED_REASON = HANDOFF_REQUESTED_REASON;
export const CONTINUATION_REPORTS_REQUESTED_REASON = REPORTS_REQUESTED_REASON;

export const CONTINUATION_EFFORT_RESTART_REASON = EFFORT_RESTART_REASON;

export const CONTINUATION_AUTO_RESUME_AFTER_API_ERROR_REASON = AUTO_RESUME_AFTER_API_ERROR_REASON;

export const IDLE_ENGINE_RESTART_INTERRUPT_REASON = ENGINE_RESTART_INTERRUPT_REASON;

export function continuationStartedSummary(
  reason: string | undefined,
  actor: MessageOrigin | undefined,
): string {
  if (reason === CONTINUATION_AUTO_RECOVERY_REASON) {
    return 'Resumed after the session stopped responding';
  }
  if (reason === CONTINUATION_AUTO_RESUME_AFTER_API_ERROR_REASON) {
    return 'Resumed after the model connection dropped';
  }
  if (reason === CONTINUATION_HARDEN_REQUESTED_REASON) return 'Hardening the work';
  if (reason === CONTINUATION_PLAN_REQUESTED_REASON) return 'Returning to the plan';
  if (reason === CONTINUATION_BUILD_REQUESTED_REASON) return 'Building the approved plan';
  if (reason === CONTINUATION_HANDOFF_REQUESTED_REASON) return 'Handing off the session';
  if (reason === CONTINUATION_REPORTS_REQUESTED_REASON) return 'Reading the waiting reports';
  if (reason === CONTINUATION_EFFORT_RESTART_REASON) return 'Restarted at the new reasoning effort';
  return originMode(actor) === 'human' ? 'Continued the response' : 'Resumed after engine restart';
}

export function continuationCause(
  reason: string | undefined,
  actor: MessageOrigin | undefined,
): string {
  if (reason === CONTINUATION_AUTO_RECOVERY_REASON) return 'Stopped responding';
  if (reason === CONTINUATION_AUTO_RESUME_AFTER_API_ERROR_REASON) return 'Connection dropped';
  if (reason === CONTINUATION_HARDEN_REQUESTED_REASON) return 'You asked to harden';
  if (reason === CONTINUATION_PLAN_REQUESTED_REASON) return 'You allowed the plan nudge';
  if (reason === CONTINUATION_BUILD_REQUESTED_REASON) return 'You allowed the build';
  if (reason === CONTINUATION_HANDOFF_REQUESTED_REASON) return 'You allowed the handoff';
  if (reason === CONTINUATION_REPORTS_REQUESTED_REASON) return 'You woke it for reports';
  if (reason === CONTINUATION_EFFORT_RESTART_REASON) return 'You changed effort';
  return originMode(actor) === 'human' ? 'You continued' : 'After restart';
}

export interface EventWaitSummary {
  wait_id: string;
  on: EventSubscription[];
  reason: string;
  expires_at: string;
  background_timing?: BackgroundWaitTiming;
}

export type StoredEvent = ThreadEvent & {
  created?: string;
  _displayCreated?: string;
  queued_for_permission?: boolean;
  _eventId?: string;
  _liveUtterance?: true;
  _liveReply?: true;
  _livePartial?: true;
  _unsent?: true;
  _answersQuestion?: string;
};

export function isChannelDefiningEvent(eventType: string): boolean {
  return eventType === 'SessionStarted'
    || eventType === 'ContinuationStarted'
    || eventType === 'TriggerStarted';
}

export type SequencedEvent = {
  seq: number;
  event: StoredEvent;
};

export type ThreadSection = 'archived' | 'inbox';
