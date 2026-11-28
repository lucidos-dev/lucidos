import { blobPreviewUrl, postCommandCheckpointUndo } from '../../api/client';
import { CONTINUE_GUARD_MS, continueStoppedThread } from './continueStoppedThread';
import type { Change } from '../../api/client';
import { ensureChangeLoaded, revertChange } from '../../store/actions/chat-changes';
import { changeCommitList, knownChangeHeadline } from '../../store/changeHeadline';
import { HELD_MESSAGE_NOTE } from '../../store/exchange-status';
import { ensureEventTargetResolved, eventHasTarget, jumpableEventId, showEventWhereItLives } from '../../store/actions/event-navigation';
import { viewChangeDiff } from '../../store/actions/repositories';
import { engineBuildDeferred, engineUnbuiltHead, checkpointDiffModal, contextViewer, eventConditionDoor, eventConditionPopoverOpenFor, findChangeById, lazyChanges, openImagePopupFromGroup, openSubAgentGroups, showToast, stepDetailModal, toggleEventConditionPopover, toggleSubAgentGroup } from '../../store/store';
import { prefetchStepDetail } from '../../store/stepDetailCache';
import { LUCIDOS_AGENT_LABEL, awaitedSubject, continuationCause, displayedWaitDeadline, plainEventName, responseAbortedState, starterEngineReason, groupSubscriptions, isRedirectCancel, isThinking, resumeEngineNote, stepStatus, subscriptionFilterNote, triggerRetrySummary } from '../../store/thread-events';
import { threadLink } from './ChildCompletionRow';
import { LucidosGlyph } from '../shared/LucidosMark';
import { BlobImage } from '../shared/BlobImage';
import { CommitList } from '../shared/CommitList';
import { Disclosure } from '../shared/Disclosure';
import { ToggleIconButton } from '../shared/ToggleIconButton';
import { DisclosureChevron } from '../shared/DisclosureChevron';
import type { AbortCause, CancelCause, ChangeLifecycleType, EngineReason, EventSubscription, EventWaitReason, EventWaitCancelCause, Exchange, MessageOrigin, StoredEvent, SubscriptionGroup } from '../../store/thread-events';
import type { Loadable, ResponseEvent, StepOutcome } from '../../store/types';
import type { FreshSessionReason } from '../../generated/thread-event-wire';
import type { CodingAgent } from '../../api/types';
import { HEARING_YOU } from '../../voice/callState';
import { describeAbortCause, describeCancelCause, describeContinuationReason, describeEngineReason, describeWatchedEvents } from '../../utils/engineEventExplainers';
import { errorDetail } from '../../utils/errorDetail';
import { formatFileCount } from '../../utils/formatFileCount';
import { formatMessageTimestamp, formatShortDate, formatShortTime, isSameDayInUserTz } from '../../utils/formatTime';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { EventRowFoldView, eventNameChip, eventRowBody } from './EventRow';
import type { EventRowChip, EventRowFact, EventRowTone } from './EventRow';
import { contextPercent, formatTokens } from '../../utils/formatTokens';
import { CallIcon, ClaudeIcon, CodexIcon, CollapseTurnIcon, FullResponseIcon, StepLogIcon, StepOutcomeIcon } from '../shared/icons';
import { highlightEllipsis } from './highlightEllipsis';
import { getSessionBlobUrlForHash } from './pastedImages';
import { useSignal } from '@preact/signals';
import type { ComponentChildren } from 'preact';
import { useEffect } from 'preact/hooks';
import type { InitiatorDescriptor } from './ChatExchange';
import { ROW_ATTR } from './scrollAnchor';
import { withScrollAnchor } from './CreateThreadView';


const CHANGE_STATE: Record<ChangeLifecycleType, { label: string; tone: EventRowTone; accent: string }> = {
  ChangeApplied: { label: 'Applied', tone: 'good', accent: 'change-applied' },
  ChangeDiscarded: { label: 'Discarded', tone: 'halted', accent: 'change-discarded' },
  ChangeReverted: { label: 'Reverted', tone: 'none', accent: 'change-reverted' },
  ChangeApplyFailed: { label: 'Failed', tone: 'bad', accent: 'change-failed' },
};

export function changeAccent(type: ChangeLifecycleType): string {
  return CHANGE_STATE[type].accent;
}

export function MarkdownBlock({ html, onClick, className }: {
  html: string;
  onClick?: (e: MouseEvent) => void;
  className?: string;
}) {
  return <div class={`markdown-content authored-html${className ? ` ${className}` : ''}`} onClick={onClick} dangerouslySetInnerHTML={{ __html: html }} />;
}

export function FileList({ files }: { files: string[] }) {
  return (
    <ul class="initiator-files">
      {files.map(f => <li key={f}><code>{f}</code></li>)}
    </ul>
  );
}

function UserImages({ imageHashes }: { imageHashes: readonly string[] }) {
  if (imageHashes.length === 0) return null;
  return (
    <div class="user-images">
      {imageHashes.map((hash, i) => {
        const src = getSessionBlobUrlForHash(hash) ?? blobPreviewUrl(hash);
        return (
          <BlobImage
            key={hash + ':' + i}
            src={src}
            class="user-image-thumb"
            alt=""
            onClick={(e) => openImagePopupFromGroup(e.currentTarget.src, e.currentTarget)}
          />
        );
      })}
    </div>
  );
}

export function UserMessageBody({ html, imageHashes }: { html: string; imageHashes: readonly string[] }) {
  return (
    <>
      {html && <div class="markdown-content authored-html" dangerouslySetInnerHTML={{ __html: html }} />}
      <UserImages imageHashes={imageHashes} />
    </>
  );
}

export function ChangeEventRow({ type, changeId, error, seedDescription, seedFileCount, seedSummary }: {
  type: ChangeLifecycleType;
  changeId?: string;
  error?: string;
  seedDescription?: string;
  seedFileCount?: number;
  seedSummary?: string;
}) {
  const change: Change | undefined = changeId ? findChangeById(changeId) : undefined;
  const lazy: Loadable<Change> = (changeId ? lazyChanges.value.get(changeId) : undefined) ?? { status: 'not-loaded' };

  useEffect(() => {
    if (changeId) void ensureChangeLoaded(changeId);
  }, [changeId]);

  const naming = change ?? { description: seedDescription, summary: seedSummary };
  const fileCount = change?.file_count ?? seedFileCount;
  const seeded = !!naming.description?.trim() || fileCount != null;

  const shownError = error
    ?? (!seeded && lazy.status === 'failed' ? `Failed to load change details: ${lazy.error}` : undefined);
  const { label, tone } = CHANGE_STATE[type];
  return changeEventRowBody({
    type,
    headline: knownChangeHeadline(naming),
    headlinePending: !change && !seeded && !!changeId && (lazy.status === 'not-loaded' || lazy.status === 'loading'),
    stateLabel: type === 'ChangeApplied' && changeWaitsForBuild(change, engineUnbuiltHead.value) && engineBuildDeferred.value
      ? appliedBuildWaitLabel(engineBuildDeferred.value.coding_threads) : label,
    tone,
    fileCount,
    commits: changeCommitList(naming),
    error: shownError,
    actions: changeActions(changeId, type === 'ChangeApplyFailed', type === 'ChangeApplied'),
  });
}

export function changeWaitsForBuild(change: Pick<Change, 'requires_restart' | 'post_merge_sha'> | undefined, unbuiltHead: string | null): boolean {
  return !!unbuiltHead && !!change?.requires_restart && change.post_merge_sha === unbuiltHead;
}

export function appliedBuildWaitLabel(count: number): string {
  return `Applied. New version waits for ${count} coding ${count === 1 ? 'thread' : 'threads'}.`;
}

export function changeEventRowBody({ type, headline, headlinePending, stateLabel, tone, fileCount, commits = [], error, actions }: {
  type: ChangeLifecycleType;
  headline?: string;
  headlinePending?: boolean;
  stateLabel: string;
  tone: EventRowTone;
  fileCount?: number;
  commits?: readonly string[];
  error?: string;
  actions?: ComponentChildren;
}) {
  return eventRowBody({
    kind: 'change',
    state: type,
    role: 'change-event-row',
    subject: eventNameChip({ kind: 'chip', name: type, label: 'change', sentenceStart: true }),
    stateLabel,
    tone,
    detail: headline ?? (headlinePending ? <span aria-hidden="true">{'\u00a0'}</span> : undefined),
    facts: [
      fileCount != null ? { kind: 'text' as const, text: formatFileCount(fileCount) } : null,
    ],
    fold: error
      ? { label: 'Error', open: true, body: error }
      : commits.length > 1
        ? { label: `${commits.length} commits`, body: <CommitList commits={commits} /> }
        : undefined,
    actions,
  });
}

export function ContinueButton({ threadId, label = 'Continue' }: { threadId: string; label?: string }) {
  const inFlight = useSignal(false);
  const onClick = async (e: MouseEvent) => {
    e.stopPropagation();
    if (inFlight.value) return;
    inFlight.value = true;
    if (!(await continueStoppedThread(threadId))) {
      inFlight.value = false;
      return;
    }
    setTimeout(() => { inFlight.value = false; }, CONTINUE_GUARD_MS);
  };
  return (
    <button class="action-btn" onClick={onClick} disabled={inFlight.value}>
      {inFlight.value ? 'Continuing...' : label}
    </button>
  );
}

const CANCEL_STATE: Partial<Record<CancelCause, string>> = {
  user_stop: 'You stopped it',
  user_action: 'Stopped by apply, discard or archive',
  superseded_by_followup: 'Replaced',
  effort_restart: 'Restarted',
  refused: 'Not allowed',
  hold: 'Stopped by a hold',
};

interface BoundaryStarter {
  type: string;
  origin?: MessageOrigin;
  actor?: MessageOrigin;
  cause?: string;
  tool?: string;
}

export type BoundaryTurn = 'waiting' | 'working' | 'done' | 'stopped';

const REASON_PILL: Partial<Record<EngineReason['kind'], string>> = {
  missing_hardening: 'hardening needed',
  missing_plan: 'plan check',
  approved_plan_unstarted: 'build the approved plan',
  context_ceiling: 'context ceiling',
  model_substituted: 'model swapped',
  harden_retrigger: 'hardening needed',
  merge_conflict: 'merge conflict',
  main_check_red: 'main is red',
  main_check_flaky: 'flaky tests',
  stale_session: 'stopped agent tidied up',
  orphan_recovery: 'picked up after a restart',
  archived_branch_work: 'work set aside',
  unmerged_integrated_work: 'work set aside',
  successor_residue: 'work set aside',
  plugin_setup: 'plugin setup',
  plugin_upstream_proposal: 'plugin change offered',
  scheduler: 'scheduled message',
};

const WAIT_REENTRY: Record<EventWaitReason['outcome'], { pill: string; meaning: string }> = {
  delivered: {
    pill: 'event arrived',
    meaning: 'Lucidos told the agent the event it was waiting for happened.',
  },
  expired: {
    pill: 'wait timed out',
    meaning: 'Lucidos told the agent its wait ran out of time.',
  },
};


const WORKING_STATE: Partial<Record<string, string>> = {
  MissingHardeningDetected: 'Running hardening',
  MergeConflictDetected: 'Resolving',
  PromptInjected: 'Agent working',
  CodingAgentPromptSent: 'Agent working',
};

function workState(type: string, turn: BoundaryTurn) {
  switch (turn) {
    case 'working': return { state: WORKING_STATE[type], tone: 'live' as const };
    case 'done': return { state: 'Done', tone: 'good' as const };
    case 'stopped': return { state: 'Stopped', tone: 'halted' as const };
    case 'waiting': return { state: 'Queued', tone: 'arrived' as const };
  }
}

function boundaryCardParts(
  ev: BoundaryStarter,
  reason: EngineReason | undefined,
  summary: string,
  pill: string,
  turn: BoundaryTurn,
) {
  const why = reason ? describeEngineReason(reason) : null;
  const fact = summary.toLowerCase() !== pill ? summary : undefined;
  switch (ev.type) {
    case 'ResponseAborted':
      return { state: responseAbortedState(ev.actor, ev.cause as AbortCause), tone: 'halted' as const, why: describeAbortCause(ev.cause as AbortCause) };
    case 'ResponseCanceled':
      return { state: CANCEL_STATE[ev.cause as CancelCause], tone: isRedirectCancel(ev.cause as CancelCause) ? 'lapsed' as const : 'halted' as const, why: describeCancelCause(ev.cause as CancelCause) };
    case 'EventWaitCanceled':
      return { state: 'You stopped it', tone: 'halted' as const, fact };
    case 'McpConsentRequested':
      return { state: 'Requested', tone: 'live' as const, fact: ev.tool };
    case 'MissingHardeningDetected':
    case 'MergeConflictDetected':
    case 'PromptInjected':
    case 'CodingAgentPromptSent':
      return { ...workState(ev.type, turn), why };
    default:
      return { tone: 'arrived' as const, why, fact };
  }
}

export function boundaryCard(event: StoredEvent, summary: string, opts: {
  carried?: ComponentChildren;
  actions?: ComponentChildren;
  turn: BoundaryTurn;
}) {
  const ev = event as BoundaryStarter;
  const reason = starterEngineReason(event);
  if (reason?.kind === 'event_wait') return waitReentryCard(reason, opts);
  const pill = ev.type === 'ResponseAborted' && ev.cause === 'stale_settle'
    ? 'stuck reply cleared'
    : (reason && REASON_PILL[reason.kind]) ?? plainEventName(ev.type);
  const { state, tone, why, fact } = {
    why: null,
    fact: undefined,
    ...boundaryCardParts(ev, reason, summary, pill, opts.turn),
  };
  return eventRowBody({
    kind: 'boundary',
    role: 'boundary-card',
    subject: eventNameChip({ kind: 'chip', name: ev.type, label: pill, sentenceStart: true }),
    stateLabel: state,
    tone,
    facts: [fact ? { kind: 'text', text: fact } : null],
    fold: why || opts.carried ? {
      label: 'Details',
      body: <>{why && <p>{why}</p>}{opts.carried}</>,
    } : undefined,
    actions: opts.actions,
  });
}

function waitReentryCard(reason: EventWaitReason, opts: { carried?: ComponentChildren; turn: BoundaryTurn }) {
  const { pill, meaning } = WAIT_REENTRY[reason.outcome];
  const waitedFor = describeWatchedEvents(reason.watched);
  const { state, tone } = workState('PromptInjected', opts.turn);
  return eventRowBody({
    kind: 'boundary',
    role: 'boundary-card',
    subject: eventNameChip({ kind: 'chip', name: 'PromptInjected', label: pill, meaning, sentenceStart: true }),
    stateLabel: state,
    tone,
    facts: [waitedFor ? { kind: 'text', text: waitedFor } : null],
    fold: {
      label: 'Details',
      body: (
        <>
          <p>{describeEngineReason(reason)}</p>
          {opts.carried && <EventRowFoldView label="What the agent was told" body={opts.carried} />}
        </>
      ),
    },
  });
}

export function ResumeCard({ exchange }: { exchange: Exchange }) {
  const ev = exchange.userEvent as { reason?: string; actor?: MessageOrigin };
  const note = resumeEngineNote(exchange);
  return eventRowBody({
    kind: 'resume',
    role: 'resume',
    subject: eventNameChip({ kind: 'chip', name: 'ContinuationStarted', sentenceStart: true }),
    stateLabel: continuationCause(ev.reason, ev.actor),
    tone: 'arrived',
    fold: {
      label: 'Details',
      body: (
        <>
          <p>{describeContinuationReason(ev.reason) ?? describeEngineReason({ kind: 'continuation_started' })}</p>
          {note && (
            <>
              <p>{note.toolCount > 0
                ? `Lucidos reminded the agent of the ${note.toolCount} step${note.toolCount === 1 ? '' : 's'} it had already taken.`
                : 'Lucidos told the agent that none of its steps had finished.'}</p>
              <EventRowFoldView label="What the agent was told" pre body={note.text} />
            </>
          )}
        </>
      ),
    },
  });
}

export function RestartRow({ event }: { event: Extract<ResponseEvent, { type: 'restart' }> }) {
  return eventRowBody({
    kind: 'resume',
    role: 'restart',
    subject: eventNameChip({ kind: 'chip', name: 'ContinuationStarted', label: 'Restarted', sentenceStart: true }),
    stateLabel: continuationCause(event.reason, event.actor),
    tone: 'arrived',
    fold: {
      label: 'Details',
      body: <p>{describeContinuationReason(event.reason) ?? describeEngineReason({ kind: 'continuation_started' })}</p>,
    },
  });
}

const FRESH_SESSION_WHY: Record<FreshSessionReason, string> = {
  'change-applied': 'The change was applied, which ends its session.',
  'change-discarded': 'The change was discarded, which ends its session.',
  'session-ended': 'The previous session ended.',
  'branch-gone': 'The branch was renamed or deleted.',
  'resume-failed': 'The previous session could not be resumed.',
  'no-resumable-session': 'No earlier session was recorded to resume.',
  'provider-blocked': 'The provider blocked the previous session, so it was not resumed.',
};

export function FreshSessionRow({ event }: { event: Extract<ResponseEvent, { type: 'fresh_session' }> }) {
  return eventRowBody({
    kind: 'session',
    role: 'fresh-session',
    state: event.reason,
    subject: eventNameChip({ kind: 'chip', name: 'SessionStarted', label: 'New session', sentenceStart: true }),
    stateLabel: 'Context not carried over',
    detail: FRESH_SESSION_WHY[event.reason],
    fold: {
      label: 'Details',
      body: <p>{'The agent started a new session without the earlier conversation. The turns above stay in the thread.'}</p>,
    },
  });
}

function handoffDetail(event: Extract<ResponseEvent, { type: 'handoff_session' }>): string {
  if (event.crossing) {
    const { context_tokens: size, ceiling } = event.crossing;
    const where = `Handed off at the context ceiling: ${formatTokens(size)} tokens, past this thread's ${formatTokens(ceiling)}.`;
    return event.opening === 'brief'
      ? `${where} The previous session did not write a handoff in its one turn, so this session started from a brief.`
      : `${where} The previous session wrote the handoff narrative.`;
  }
  const at = [event.checkpoint, event.progress].filter(Boolean).join(' ');
  return at ? `Carried on from a handoff at ${at}.` : 'Carried on from a handoff.';
}

export function HandoffSessionRow({ event }: { event: Extract<ResponseEvent, { type: 'handoff_session' }> }) {
  return eventRowBody({
    kind: 'session',
    role: 'handoff-session',
    subject: eventNameChip({ kind: 'chip', name: 'SessionStarted', label: 'New session', sentenceStart: true }),
    stateLabel: event.opening === 'brief' ? 'Handed off from a brief' : 'Handed off',
    detail: handoffDetail(event),
    fold: {
      label: 'What the new session was told',
      body: <MarkdownBlock html={renderMarkdown(event.message)} />,
    },
  });
}

export function EventDeliveryBody({
  eventType,
  eventId,
  payloadJson,
  heldNote,
}: {
  eventType: string;
  eventId?: string;
  payloadJson?: string;
  heldNote?: string;
}) {
  const jump = useEventJump(jumpableEventId(eventId, eventType));
  return eventDeliveryBody({
    eventType,
    payloadJson,
    opening: jump.opening,
    onOpenMatched: jump.onOpen,
    heldNote,
  });
}

function useEventTarget(eventId: string | undefined): boolean {
  useEffect(() => { ensureEventTargetResolved(eventId); }, [eventId]);
  return eventHasTarget(eventId);
}

function useEventJump(eventId: string | undefined): { opening: boolean; onOpen?: () => void } {
  const opening = useSignal(false);
  const linkable = useEventTarget(eventId);
  if (!linkable || !eventId) return { opening: opening.value };
  return {
    opening: opening.value,
    onOpen: async () => {
      if (opening.value) return;
      opening.value = true;
      try {
        await showEventWhereItLives(eventId);
      } finally {
        opening.value = false;
      }
    },
  };
}

export function eventDeliveryBody({
  eventType,
  payloadJson,
  opening,
  onOpenMatched,
  heldNote,
}: {
  eventType: string;
  payloadJson?: string;
  opening: boolean;
  onOpenMatched?: () => void;
  heldNote?: string;
}) {
  return eventRowBody({
    kind: 'delivery',
    heldNote,
    role: 'event-delivery',
    subject: eventNameChip({
      kind: 'chip',
      name: eventType,
      sentenceStart: true,
      onClick: onOpenMatched,
      pending: opening,
      role: 'event-delivery-jump',
    }),
    stateLabel: 'Arrived',
    tone: 'arrived',
    fold: payloadJson ? { label: 'Details', pre: true, body: payloadJson } : undefined,
  });
}

type TriggerStartedEvent = Extract<Exchange['userEvent'], { type: 'TriggerStarted' }>;

export function TriggerFiredBody({ event }: { event: TriggerStartedEvent }) {
  const invocation = event.invocation;
  const matched = invocation?.kind === 'Event'
    ? jumpableEventId(invocation.event_id, invocation.event_type)
    : undefined;
  const jump = useEventJump(matched);
  return triggerFiredBody({
    event,
    opening: jump.opening,
    onOpenMatched: jump.onOpen,
  });
}

export function triggerFiredBody({
  event,
  opening,
  onOpenMatched,
}: {
  event: TriggerStartedEvent;
  opening: boolean;
  onOpenMatched?: () => void;
}) {
  const invocation = event.invocation;
  const name = event.trigger_name?.trim();
  return eventRowBody({
    kind: 'trigger',
    role: 'trigger-fired',
    subject: eventNameChip({ kind: 'chip', name: 'TriggerStarted', sentenceStart: true }),
    stateLabel: invocation?.kind === 'Schedule' ? 'Scheduled' : invocation?.kind === 'Event' ? 'On event' : 'Fired',
    tone: 'arrived',
    facts: [
      name ? { kind: 'text' as const, text: name } : null,
      event.retry_of
        ? { kind: 'node' as const, node: threadLink(event.retry_of.thread_id, 'Retry of an interrupted run') }
        : null,
      invocation?.kind === 'Event'
        ? {
            kind: 'chip' as const,
            name: invocation.event_type,
            onClick: onOpenMatched,
            pending: opening,
            role: 'trigger-event-jump',
          }
        : null,
    ],
    fold: event.prompt
      ? { label: 'Details', body: <MarkdownBlock html={renderMarkdown(event.prompt)} /> }
      : undefined,
  });
}

export function TriggerInterruptionNotice({ event }: { event: Extract<StoredEvent, { type: 'TriggerRunInterrupted' }> }) {
  return eventRowBody({
    kind: 'trigger',
    role: 'trigger-interrupted',
    subject: 'An engine restart interrupted this run',
    detail: triggerRetrySummary(event.retry),
  });
}

export function changeActions(changeId?: string, suppress?: boolean, reserveWhileLoading?: boolean): ComponentChildren {
  if (suppress || !changeId) return null;
  const change = findChangeById(changeId);
  if (!change) {
    return reserveWhileLoading
      ? <button class="action-btn change-actions-placeholder" aria-hidden="true" tabIndex={-1}>Revert</button>
      : null;
  }
  const showDiff = change.status === 'pending' || !!change.pre_merge_sha;
  const showRevert = change.status === 'applied';
  if (!showDiff && !showRevert) return null;
  return (
    <>
      {showDiff && <button class="action-btn" onClick={() => void viewChangeDiff(change)}>Diff</button>}
      {showRevert && <button class="action-btn action-btn-danger" onClick={() => void revertChange(change.id)}>Revert</button>}
    </>
  );
}

function collapseControl(
  { collapsed, collapsible, onToggle }: { collapsed: boolean; collapsible: boolean; onToggle: (e: MouseEvent) => void },
): ComponentChildren {
  const label = collapsed ? 'Expand this turn' : 'Collapse this turn';
  return (
    <ToggleIconButton
      tone="glyph"
      class="turn-control-collapse"
      data-role="toggle-collapsed"
      disabled={!collapsible}
      pressed={collapsed}
      aria-label={label}
      data-tooltip={label}
      onClick={onToggle}
    >
      <CollapseTurnIcon collapsed={collapsed} />
    </ToggleIconButton>
  );
}

interface InitiatorPanelProps {
  initiator: InitiatorDescriptor;
  timestamp: string;
  onActorClick?: (e: MouseEvent) => void;
  collapsible: boolean;
  collapsed: boolean;
  onToggle?: (e: MouseEvent) => void;
  onBodyClick?: (e: MouseEvent) => void;
  bubble?: boolean;
  teammate?: string | null;
  chromeless?: boolean;
  held?: { note?: string };
}

function ActorChipBody({ initiator }: { initiator: InitiatorDescriptor }) {
  return (
    <>
      {initiator.icon != null && initiator.icon !== '' && (
        <span class="initiator-icon">{initiator.icon}</span>
      )}
      <span class="initiator-label">{initiator.label}</span>
    </>
  );
}

export function InitiatorPanel({ initiator, timestamp, onActorClick, collapsible, collapsed, onToggle, onBodyClick, bubble = false, teammate = null, chromeless = false, held }: InitiatorPanelProps) {
  const accentClass = initiator.accent ? ` initiator-panel-${initiator.accent}` : '';
  const hasBody = !!initiator.summary || !!initiator.details;

  const collapse = collapsible && onToggle
    ? <span class="turn-controls">{collapseControl({ collapsed, collapsible, onToggle })}</span>
    : null;

  return (
    <div class={`initiator-panel initiator-panel-${initiator.variant}${accentClass}${bubble ? ' initiator-panel-bubble' : ''}${teammate ? ' initiator-panel-teammate' : ''}`} data-held={held ? '' : undefined}>
      <div class="initiator-header">
        {!chromeless && (onActorClick ? (
          <button
            type="button"
            class="initiator-actor"
            onClick={onActorClick}
            aria-label={`Show info for ${initiator.label}`}
          >
            <ActorChipBody initiator={initiator} />
          </button>
        ) : (
          <span class="initiator-actor">
            <ActorChipBody initiator={initiator} />
          </span>
        ))}
        {collapse}
        <span class={`initiator-meta${teammate ? ' initiator-meta-teammate' : ''}`}>
          {teammate && <span class="initiator-sender">{teammate}</span>}
          {teammate && <span class="initiator-sender-dot" aria-hidden="true">{'·'}</span>}
          {initiator.status}
          {chromeless && onActorClick ? (
            <button
              type="button"
              class="initiator-timestamp initiator-timestamp-button"
              onClick={onActorClick}
              aria-label={`Show info for ${initiator.label}`}
            >
              {timestamp}
            </button>
          ) : (
            <span class="initiator-timestamp">{timestamp}</span>
          )}
        </span>
      </div>
      {hasBody && (
        <Disclosure open={!collapsed}>
          <div class="initiator-body" onClick={onBodyClick}>
            {initiator.summary && <div class="initiator-summary">{initiator.summary}</div>}
            {bubble ? <div class="user-bubble">{initiator.details}</div> : initiator.details}
            {held?.note && <div class="initiator-held-note">{held.note}</div>}
          </div>
        </Disclosure>
      )}
    </div>
  );
}


export function describeExecutor(
  isCC: boolean,
  codingAgent: CodingAgent = 'claude-code',
): { icon: ComponentChildren; label: string } {
  if (isCC && codingAgent === 'codex') return { icon: <CodexIcon />, label: 'Codex' };
  if (isCC) return { icon: <ClaudeIcon />, label: 'Claude' };
  return { icon: <LucidosGlyph />, label: LUCIDOS_AGENT_LABEL };
}

interface TurnControlsProps {
  detailsOn: boolean;
  stepsOn: boolean;
  collapsed: boolean;
  collapsible: boolean;
  onToggleDetails: (e: MouseEvent) => void;
  onToggleSteps: (e: MouseEvent) => void;
  onToggleCollapsed: (e: MouseEvent) => void;
}

export function turnControls({
  detailsOn, stepsOn, collapsed, collapsible, onToggleDetails, onToggleSteps, onToggleCollapsed,
}: TurnControlsProps): ComponentChildren {
  const detailsLabel = detailsOn ? 'Show the latest answer only' : 'Show the full response';
  const stepsLabel = stepsOn ? 'Hide steps' : 'Show steps';
  return (
    <span class="turn-controls">
      <ToggleIconButton
        tone="quiet"
        data-role="toggle-details"
        pressed={detailsOn}
        aria-label={detailsLabel}
        data-tooltip={detailsLabel}
        onClick={onToggleDetails}
      >
        <FullResponseIcon />
      </ToggleIconButton>
      <ToggleIconButton
        tone="quiet"
        data-role="toggle-steps"
        pressed={stepsOn}
        aria-label={stepsLabel}
        data-tooltip={stepsLabel}
        onClick={onToggleSteps}
      >
        <StepLogIcon />
      </ToggleIconButton>
      {collapseControl({ collapsed, collapsible, onToggle: onToggleCollapsed })}
    </span>
  );
}

interface ResponsePanelProps {
  executor: { icon: ComponentChildren; label: string };
  onExecutorClick?: (e: MouseEvent) => void;
  controls?: ComponentChildren;
  status: ComponentChildren;
  timestamp: string;
  collapsed: boolean;
  hasBody: boolean;
  headerless?: boolean;
  children: ComponentChildren;
}

export function ResponsePanel({
  executor, onExecutorClick, controls, status, timestamp, collapsed, hasBody, headerless = false, children,
}: ResponsePanelProps) {
  const folded = collapsed && !headerless;
  return (
    <div class="response-panel">
      {!headerless && (
        <div class="response-header">
          <button
            type="button"
            class="response-executor"
            onClick={onExecutorClick}
            aria-label="Show who answered"
          >
            <span class="response-executor-icon">{executor.icon}</span>
            <span class="response-executor-label">{executor.label}</span>
          </button>
          {controls}
          <span class="response-meta">
            {status}
            <span class="response-timestamp">{timestamp}</span>
          </span>
        </div>
      )}
      <Disclosure open={hasBody && !folded}>
        <div class="response-body">
          {children}
        </div>
      </Disclosure>
    </div>
  );
}

function clip(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

const IMAGE_PROMPT_SUMMARY_MAX = 240;

export function imagePromptSummary(prompt: string | undefined, max = IMAGE_PROMPT_SUMMARY_MAX): string | undefined {
  const text = prompt?.replace(/\s+/g, ' ').trim();
  if (!text) return undefined;
  return clip(text, max);
}

export function GeneratedImage({ event }: { event: Extract<ResponseEvent, { type: 'image' }> }) {
  const src = `data:${event.mime_type};base64,${event.base64}`;
  const summary = imagePromptSummary(event.prompt);
  return (
    <div class="generated-image">
      <img
        src={src}
        class="image-thumbnail"
        alt={summary ?? 'Generated image'}
        data-tooltip={summary}
        loading="lazy"
      />
    </div>
  );
}

function lastLinePreview(text: string, max = 120): string {
  const line = text.split('\n').map(s => s.trim()).filter(Boolean).pop() ?? '';
  return clip(line, max);
}

export function contextLabel(
  used: number,
  window: number | null | undefined,
  messages: number | null | undefined,
  compact: boolean,
): string {
  if (window) {
    const pct = `${contextPercent(used, window)}%`;
    return compact ? pct : `${formatTokens(used)} / ${formatTokens(window)} (${pct})`;
  }
  if (compact) return formatTokens(used);
  return `${formatTokens(used)} tokens${messages != null ? `, ${messages} msgs` : ''}`;
}

const NAMED_STEP_OUTCOMES: ReadonlySet<StepOutcome> = new Set<StepOutcome>([
  'error',
  'unfinished',
  'blocked',
  'denied',
]);

export function stepRowText(event: Extract<ResponseEvent, { type: 'step' }>): { description: string; detail: string } {
  const thinkingTail = isThinking(event) && event.thinkingText
    ? lastLinePreview(event.thinkingText)
    : '';
  return { description: event.description, detail: event.detail || thinkingTail };
}

export function InlineStep(
  { event, rowRef, fold, anchored = true }: {
    event: Extract<ResponseEvent, { type: 'step' }>;
    rowRef?: (el: HTMLDivElement | null) => void;
    fold?: StepFold;
    anchored?: boolean;
  },
) {
  const { label, className } = stepStatus(event.outcome);
  const { description, detail: detailText } = stepRowText(event);

  const isPending = event.outcome === 'pending';

  return (
    <div
      ref={rowRef}
      class={`inline-step ${className}${fold ? ' has-fold' : ''}`}
      data-role="inline-step"
      data-find-row={event.seq}
      {...(anchored ? { [ROW_ATTR]: event.call_event_id ?? event.result_event_id } : {})}
      data-thinking-row={isThinking(event) ? '' : undefined}
      data-tooltip={NAMED_STEP_OUTCOMES.has(event.outcome) ? label : undefined}
    >
      {fold && (
        <button
          type="button"
          class="step-fold"
          data-role="step-fold"
          aria-expanded={fold.open}
          aria-label={`${fold.open ? 'Hide' : 'Show'} the agent's ${stepCount(fold.count)}`}
          onClick={fold.onToggle}
        >
          <DisclosureChevron open={fold.open} />
        </button>
      )}
      <button
        type="button"
        class="step-main"
        data-role="step-main"
        onPointerDown={() => prefetchStepDetail(event)}
        onClick={() => { stepDetailModal.value = event; }}
      >
        {!fold && <span class="step-icon"><StepOutcomeIcon outcome={event.outcome} /></span>}
        <span class={`step-description${isPending ? ' running-shimmer' : ''}`}>{highlightEllipsis(description)}</span>
        {detailText && <span class="step-detail">{highlightEllipsis(detailText)}</span>}
      </button>
      {fold && (
        <button
          type="button"
          class="step-fold-count"
          tabIndex={-1}
          aria-hidden="true"
          onClick={fold.onToggle}
        >
          {stepCount(fold.count)}
        </button>
      )}
      <StepContext event={event} />
    </div>
  );
}

export function StepContext({ event }: { event: Extract<ResponseEvent, { type: 'step' }> }) {
  const snap = event.contextCapture;
  const used = snap?.usage?.input_tokens ?? snap?.estimated_total_tokens ?? event.context_tokens;
  if (used == null) return null;
  const window = snap?.context_window;
  const trimmed = snap?.trimmed ?? event.trimmed;
  const counter = (
    <>
      <span class="step-context-full">
        {contextLabel(used, window, event.context_messages, false)}
      </span>
      <span class="step-context-compact">
        {contextLabel(used, window, event.context_messages, true)}
      </span>
      {trimmed && ' · trimmed'}
    </>
  );
  return snap ? (
    <button
      type="button"
      class={`step-context${trimmed ? ' trimmed' : ''}`}
      data-role="step-context"
      aria-label="Show the context sent for this call"
      onClick={() => { contextViewer.value = { snapshot: snap, description: event.description }; }}
    >
      {counter}
    </button>
  ) : (
    <span class={`step-context${trimmed ? ' trimmed' : ''}`}>{counter}</span>
  );
}

interface StepFold {
  open: boolean;
  count: number;
  onToggle: (e: MouseEvent) => void;
}

const stepCount = (n: number) => `${n} ${n === 1 ? 'step' : 'steps'}`;

export function SubAgentStepGroup(
  { event, rowRef }: {
    event: Extract<ResponseEvent, { type: 'step' }>;
    rowRef?: (el: HTMLDivElement | null) => void;
  },
) {
  const children = event.children ?? [];
  const id = event.tool_use_id ?? '';
  const open = openSubAgentGroups.value.has(id);
  const running = event.outcome === 'pending';
  const onToggle = (e: MouseEvent) =>
    withScrollAnchor(e.currentTarget as HTMLElement | null, () => toggleSubAgentGroup(id));
  return (
    <div class="sub-agent-group" data-role="sub-agent-group">
      <InlineStep
        event={event}
        rowRef={rowRef}
        fold={{ open, count: children.length, onToggle }}
      />
      <Disclosure open={!open}>
        <div class={`sub-agent-tail${running ? '' : ' ended'}`} data-role="sub-agent-tail">
          {running
            ? <InlineStep event={children[children.length - 1]} anchored={false} />
            : <SubAgentEnding event={event} />}
        </div>
      </Disclosure>
      <Disclosure open={open}>
        <div class="sub-agent-steps" data-role="sub-agent-steps">
          {children.map((child, i) => <InlineStep key={child.tool_use_id ?? i} event={child} anchored={false} />)}
        </div>
      </Disclosure>
    </div>
  );
}

function SubAgentEnding({ event }: { event: Extract<ResponseEvent, { type: 'step' }> }) {
  const { label, className } = stepStatus(event.outcome);
  const final = event.finalContextCapture;
  return (
    <div class={`inline-step ${className}`}>
      <button
        type="button"
        class="step-main"
        onPointerDown={() => prefetchStepDetail(event)}
        onClick={() => { stepDetailModal.value = event; }}
      >
        <span class="step-icon"><StepOutcomeIcon outcome={event.outcome} /></span>
        <span class="step-description">{event.outcome === 'success' ? 'Done' : label}</span>
      </button>
      {final && (
        <StepContext
          event={{ type: 'step', description: event.description, outcome: event.outcome, contextCapture: final }}
        />
      )}
    </div>
  );
}

type EventWaitState = Extract<ResponseEvent, { type: 'event_wait' }>['state'];

const EVENT_WAIT_ROW_TONE: Record<EventWaitState, EventRowTone> = {
  waiting: 'live',
  matched: 'arrived',
  timed_out: 'lapsed',
  canceled: 'halted',
};

function eventWaitStateLabel(state: EventWaitState, expiresAt: string, matchedAt?: string): string {
  const deadline = waitMoment(expiresAt);
  switch (state) {
    case 'waiting':
      return deadline ? `Until ${deadline}` : 'Waiting';
    case 'matched': {
      const done = waitMoment(matchedAt);
      return done ? `Arrived ${done}` : 'Arrived';
    }
    case 'timed_out':
      return deadline ? `Gave up at ${deadline}` : 'Gave up';
    case 'canceled':
      return 'Stopped';
  }
}

const EVENT_WAIT_STOP_NOTE: Record<EventWaitCancelCause, string | null> = {
  user_stop: 'you stopped it',
  agent_stand_down: 'the agent stopped it',
  thread_archived: 'stopped when the thread was archived',
  thread_discarded: 'stopped when the thread was discarded',
  thread_canceled: 'stopped by Stop',
  snapshot_restored: 'stopped by a workspace restore',
  unknown: null,
};

export function SpokenChip() {
  return (
    <span class="spoken-chip" data-role="spoken-chip">
      <CallIcon />
      <span class="visually-hidden">{'Spoken'}</span>
    </span>
  );
}

export function SpokenReply(
  { event }: { event: Extract<ResponseEvent, { type: 'spoken_reply' }> },
) {
  return (
    <div class="spoken-reply" data-role="spoken-reply" data-find-row={event.seq}>
      <span class="spoken-reply-who" aria-hidden={event.follows ? 'true' : undefined}>
        {!event.follows && <CallIcon />}
        {!event.follows && <span class="visually-hidden">{'Said aloud'}</span>}
      </span>
      <span class="spoken-reply-text">
        <span class="spoken-reply-words">{event.text}</span>
        {event.interrupted && <span class="spoken-reply-cut">{'cut off'}</span>}
      </span>
    </div>
  );
}

export function LiveSpeechMark() {
  return (
    <span class="live-speech-mark" data-role="live-speech-mark" aria-hidden="true">
      <span class="live-speech-bar" />
      <span class="live-speech-bar" />
      <span class="live-speech-bar" />
    </span>
  );
}

export function LivePartialBody({ text }: { text: string }) {
  return (
    <span class="live-partial" data-role="live-partial">
      {text}
      <LiveSpeechMark />
      <span class="visually-hidden">{HEARING_YOU}</span>
    </span>
  );
}

export function LiveUtteranceBody() {
  return (
    <span class="live-utterance" data-role="live-utterance">
      <LiveSpeechMark />
      <span class="visually-hidden">{HEARING_YOU}</span>
    </span>
  );
}

export function EventWaitRow({ event }: { event: Extract<ResponseEvent, { type: 'event_wait' }> }) {
  return eventWaitRowBody({ event });
}

export function eventWaitRowBody({
  event,
}: {
  event: Extract<ResponseEvent, { type: 'event_wait' }>;
}) {
  const stopNote = event.state === 'canceled' && event.cause ? EVENT_WAIT_STOP_NOTE[event.cause] : null;
  return eventRowBody({
    kind: 'wait',
    state: event.state,
    role: 'event-wait-row',
    findRow: event.seq,
    subject: eventNameChip({ kind: 'chip', name: 'EventWaitStarted', label: WAIT_PILL }),
    stateLabel: eventWaitStateLabel(
      event.state,
      event.state === 'waiting' ? displayedWaitDeadline(event) : event.expires_at,
      event.matched_at,
    ),
    tone: EVENT_WAIT_ROW_TONE[event.state],
    detail: waitDetail(event.reason),
    time: eventWaitTime(event.created),
    facts: event.state === 'waiting'
      ? subscriptionFacts(event.wait_id, event.subscriptions)
      : [stopNote ? { kind: 'text' as const, text: stopNote } : null],
  });
}

export const WAIT_PILL = 'Waiting for';

export function waitDetail(reason: string): string {
  const subject = awaitedSubject(reason).trim();
  return subject ? subject.charAt(0).toUpperCase() + subject.slice(1) : 'An event';
}

export function HeldMessageRow({ event }: { event: Extract<ResponseEvent, { type: 'held_message' }> }) {
  return eventRowBody({
    kind: 'held',
    state: event.released ? 'released' : 'held',
    role: 'held-message-row',
    subject: eventNameChip({ kind: 'chip', name: 'MessageHeld', sentenceStart: true }),
    stateLabel: event.released ? HELD_MESSAGE_DELIVERED : undefined,
    tone: event.released ? 'arrived' : 'none',
    heldNote: event.released ? undefined : HELD_MESSAGE_NOTE,
    facts: [{ kind: 'text', text: `from ${event.sender}` }],
    fold: { label: 'Details', body: event.text },
  });
}

export const HELD_MESSAGE_DELIVERED = 'Delivered';

function subscriptionFacts(waitId: string, subscriptions: EventSubscription[]): EventRowFact[] {
  const chip = (g: SubscriptionGroup): EventRowChip => {
    const name = g.event_type;
    const note = subscriptionFilterNote(g);
    const door = eventConditionDoor(g);
    if (!door) return { kind: 'chip', name, note };
    return {
      kind: 'chip',
      name,
      note,
      action: door.label,
      popupOpen: eventConditionPopoverOpenFor(waitId, name),
      onClick: (anchor) => toggleEventConditionPopover({ ...door.condition, anchor, waitId }),
    };
  };
  return groupSubscriptions(subscriptions).flatMap((g, i) => [
    { kind: 'glue' as const, text: i === 0 ? 'watching for' : 'or' },
    chip(g),
  ]);
}

function eventWaitTime(created: string | undefined): { label: string; iso: string } | undefined {
  if (!created || Number.isNaN(new Date(created).getTime())) return undefined;
  return { label: formatMessageTimestamp(created), iso: created };
}

function waitMoment(iso: string | undefined): string | undefined {
  if (!iso) return undefined;
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return undefined;
  return isSameDayInUserTz(at, new Date())
    ? formatShortTime(at)
    : `${formatShortDate(at)} ${formatShortTime(at)}`;
}

export function checkpointUndoScope(restores: number, removes: number): string | null {
  const parts: string[] = [];
  if (restores > 0) parts.push(`restore ${formatFileCount(restores)}`);
  if (removes > 0) parts.push(`remove ${formatFileCount(removes)} this step created`);
  return parts.length > 0 ? `Undo will ${parts.join(' and ')}.` : null;
}

export function CheckpointCard({ event }: { event: Extract<ResponseEvent, { type: 'checkpoint' }> }) {
  const pending = useSignal(false);
  const onUndo = async () => {
    if (pending.value || event.reverted) return;
    pending.value = true;
    try {
      await postCommandCheckpointUndo(event.checkpoint_id);
    } catch (e) {
      showToast('Undo failed: ' + errorDetail(e), 'error');
    } finally {
      pending.value = false;
    }
  };
  const scope = checkpointUndoScope(event.restores, event.removes);
  return (
    <div class="step-note-card checkpoint-card" data-role="checkpoint-card" data-find-row={event.seq}>
      <span class="step-note-icon" aria-hidden="true">{'⎌'}</span>
      <div class="step-note-body">
        <div class="step-note-summary">{event.summary}</div>
        <code class="step-note-detail">{event.command}</code>
        {scope && <div class="checkpoint-scope">{scope}</div>}
      </div>
      <div class="checkpoint-actions">
        <button
          type="button"
          class="action-btn"
          data-role="checkpoint-diff"
          onClick={() => { checkpointDiffModal.value = event; }}
        >
          Diff
        </button>
        {event.reverted ? (
          <span class="checkpoint-reverted">Reverted ✓</span>
        ) : (
          <button
            type="button"
            class="action-btn action-btn-danger"
            data-role="checkpoint-undo"
            disabled={pending.value}
            onClick={onUndo}
          >
            {pending.value ? 'Undoing…' : 'Undo'}
          </button>
        )}
      </div>
    </div>
  );
}
