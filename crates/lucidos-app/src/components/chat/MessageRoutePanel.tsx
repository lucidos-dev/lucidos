import { useRef } from 'preact/hooks';
import { messageRoutePanel, closeMessageRoutePanel, triggers, threadMap, repositories } from '../../store/store';
import { focusThread } from '../../store/actions/threads';
import { navigateToTrigger } from '../../store/actions/triggers';
import { loadRepositories } from '../../store/actions/chat';
import { useDismissOnOutside, useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { loadedOr } from '../../store/types';
import {
  exchangeResponseModel,
  exchangeReasoningEffort,
  displayModelName,
  displayReasoningEffort,
  findPermissionResolution,
  findQuestionAnswer,
  isChangeLifecycleEvent,
  legacyOrigin,
  PENDING_TITLE_PLACEHOLDER,
  sortEventsChronologically,
  type Exchange,
  type EngineReason,
  type MessageOrigin,
  type StoredEvent,
} from '../../store/thread-events';
import { describeEngineReason } from '../../utils/engineEventExplainers';

/** Takes the full Exchange so the divider-starter cases (UserQuestionAsked,
 *  CodingAgentPermissionRequest) can walk the exchange's steps for the matching
 *  resolution event and read the device actor from there. */
export function resolveOrigin(exchange: Exchange): MessageOrigin | undefined {
  const userEvent = exchange.userEvent;
  if (userEvent.type === 'MessageReceived') return legacyOrigin(userEvent);
  if (isChangeLifecycleEvent(userEvent)) return userEvent.actor;
  // Divider-starter ActionRequired events: the Origin is the device that
  // *answered* the question / *resolved* the permission — pulled from the
  // matching resolution step in this exchange. No answer yet → undefined and
  // the popover degrades to the initiator row only.
  if (userEvent.type === 'UserQuestionAsked') {
    return findQuestionAnswer(exchange, userEvent.tool_use_id)?.actor;
  }
  if (userEvent.type === 'CodingAgentPermissionRequest') {
    return findPermissionResolution(exchange, userEvent.request_id)?.actor;
  }
  // CredentialRequested / McpConsentRequested have no user-side answer event
  // today — the initiator row carries the disclosure on its own.
  if (userEvent.type === 'CredentialRequested' || userEvent.type === 'McpConsentRequested') {
    return undefined;
  }
  // Engine-emitted events (ContinuationStarted, CodingAgentPromptSent, TriggerStarted, ChangeProposed)
  // carry origin directly — surface it so the popover can render the Engine variant.
  if ('origin' in userEvent && userEvent.origin) {
    return userEvent.origin as MessageOrigin;
  }
  // ContinuationStarted triggered by the user clicking Continue stamps the device
  // on `EventMeta.actor` (the chip reads it for the "You" label) but leaves
  // `origin` empty — fall back so the popover matches the chip.
  if ('actor' in userEvent && userEvent.actor) {
    return userEvent.actor as MessageOrigin;
  }
  return undefined;
}

/** Live lookup wins over `cachedLinkedTitle` because SSE handlers
 *  (`CodingAgentThreadSpawned`, the skeleton path in `handleThreadEvent`)
 *  create the linked thread without metadata, so the cache stays undefined
 *  until the next 5s `loadAllThreads` poll — without this fallback the
 *  popover shows the linked thread's UUID for that window. */
export function resolveThreadLinkTitle(
  origin: Extract<MessageOrigin, { kind: 'thread_link' }>,
  cachedLinkedTitle: string | undefined,
  getLiveTitle: (threadId: string) => string | undefined,
): string {
  const live = getLiveTitle(origin.thread_id);
  if (live && live !== PENDING_TITLE_PLACEHOLDER) return live;
  if (origin.title) return origin.title;
  if (cachedLinkedTitle) return cachedLinkedTitle;
  return origin.thread_id;
}

/** Branch + ccSessionId come from `SessionStarted` (or `ContinuationStarted`), which fire
 *  once per CC process spawn — not per user message. A follow-up exchange within an
 *  existing CC session has no SessionStarted in its own steps, so we walk the full thread
 *  up to this exchange's user event and track the most recent branch-defining event.
 *
 *  Permission/context info is per-exchange and stays scoped to `exchange.steps`. */
export function executorExtras(
  exchange: Exchange,
  threadEvents: Map<number, StoredEvent>,
): {
  branch?: string;
  permissionMode?: string;
  ccSessionId?: string;
  contextTokens?: number;
  contextTrimmed?: boolean;
  repoId?: string;
} {
  // The cutoff is the last seq in this exchange — SessionStarted typically lives in
  // `steps` right after the userEvent, so breaking at `userSeq` would exit too early.
  let lastSeq = exchange.userSeq;
  for (const { seq } of exchange.steps) {
    if (seq > lastSeq) lastSeq = seq;
  }

  let branch: string | undefined;
  let ccSessionId: string | undefined;
  let repoId: string | undefined;
  for (const { seq, event } of sortEventsChronologically(threadEvents)) {
    if (event.type === 'SessionStarted') {
      if (event.branch) branch = event.branch;
      if (event.session_id) ccSessionId = event.session_id;
      // Each SessionStarted snapshots its own repo bind — don't fall back to a
      // prior session's repo_id when the current SessionStarted lacks one
      // (legacy events from before the engine always stamped repo_id).
      repoId = event.repo_id;
    } else if (event.type === 'ContinuationStarted' && event.branch) {
      branch = event.branch;
    }
    if (seq === lastSeq) break;
  }

  let permissionMode: string | undefined;
  let contextTokens: number | undefined;
  let contextTrimmed: boolean | undefined;
  for (const { event } of exchange.steps) {
    if (event.type === 'CodingAgentSettingsChanged') {
      if (event.permission_mode) permissionMode = event.permission_mode;
    } else if (event.type === 'Thinking') {
      // Legacy DB rows — ContextCaptured below overrides when present.
      if (typeof event.context_tokens === 'number') contextTokens = event.context_tokens;
      if (typeof event.trimmed === 'boolean') contextTrimmed = event.trimmed;
    } else if (event.type === 'ContextCaptured') {
      // Prefer real input_tokens; fall back to the chars/4 estimate.
      const tokens = event.usage?.input_tokens ?? event.estimated_total_tokens;
      if (typeof tokens === 'number') contextTokens = tokens;
      if (typeof event.trimmed === 'boolean') contextTrimmed = event.trimmed;
    }
  }
  return { branch, permissionMode, ccSessionId, contextTokens, contextTrimmed, repoId };
}

export function MessageRoutePanel() {
  const state = messageRoutePanel.value;
  const ref = useRef<HTMLDivElement>(null);
  // Clamp horizontally to the chat pane that owns the anchor so the popover
  // stays inside the chat column instead of overflowing into the content pane.
  const pos = useAnchoredPosition(state?.anchor ?? null, ref, '.thread-pane');

  useDismissOnOutside(state !== null, ref, state?.anchor ?? null, closeMessageRoutePanel);

  if (!state) return null;
  const { exchange, threadId, section, priorModel, priorEffort } = state;
  const thread = threadMap.value.get(threadId);
  if (!thread) return null;

  return (
    <div
      ref={ref}
      class={`message-route-panel ${pos?.placement ?? ''}`}
      style={pos ? { top: `${pos.top}px`, left: `${pos.left}px` } : { visibility: 'hidden' }}
      role="dialog"
      aria-label={section === 'origin' ? 'Initiator info' : 'Executor info'}
    >
      {section === 'origin'
        ? renderOriginSection(exchange, thread.meta.parentThreadTitle, getLiveThreadTitle)
        : renderExecutorSection(exchange, thread.events, priorModel, priorEffort)}
    </div>
  );
}

function getLiveThreadTitle(threadId: string): string | undefined {
  return threadMap.value.get(threadId)?.meta.title;
}

function renderOriginSection(
  exchange: Exchange,
  parentTitle: string | undefined,
  getLiveTitle: (threadId: string) => string | undefined,
) {
  const userEvent = exchange.userEvent;
  // TriggerStarted keeps its richer renderer (invocation kind + event link).
  if (userEvent.type === 'TriggerStarted') {
    return (
      <section class="route-section">
        <h4>Origin</h4>
        {renderTriggerOrigin(userEvent)}
      </section>
    );
  }

  const initiatorRow = renderInitiatorRow(userEvent);
  const origin = resolveOrigin(exchange);
  const channel = origin ? renderChannelSection(origin, parentTitle, getLiveTitle) : null;
  const audit = origin ? renderAuditSection(origin) : null;
  const explainer = origin?.kind === 'engine'
    ? renderEngineExplainerSection(origin.reason)
    : null;

  if (!initiatorRow && !channel && !audit && !explainer) {
    return (
      <section class="route-section">
        <h4>Origin</h4>
        <div class="muted">Unknown</div>
      </section>
    );
  }

  return (
    <section class="route-section">
      <h4>Origin</h4>
      {initiatorRow}
      {channel}
      {audit}
      {explainer}
    </section>
  );
}

/** Initiator row for divider-starter ActionRequired events. The chip itself
 *  reads "You" (the device-owner who answers); this row discloses who *asked*
 *  — Claude Code for question/permission prompts, Lucidos for credential and
 *  MCP-consent requests. Returns null for non-divider events (their initiator
 *  is implied by the channel/audit rows). */
export function renderInitiatorRow(userEvent: StoredEvent): preact.JSX.Element | null {
  const text = initiatorRowText(userEvent);
  if (!text) return null;
  return (
    <div class="route-row">
      <strong>Asked by</strong>
      <span>{text}</span>
    </div>
  );
}

function initiatorRowText(userEvent: StoredEvent): string | null {
  switch (userEvent.type) {
    case 'UserQuestionAsked':            return 'Claude Code';
    case 'CodingAgentPermissionRequest': return 'Claude Code (permission gate)';
    case 'CredentialRequested':          return 'Lucidos (credential request)';
    case 'McpConsentRequested':          return 'Lucidos (tool consent)';
    default:                             return null;
  }
}

/** Channel: who, on what surface — device label / API user-agent / workspace
 *  name / parent thread title. Returns null for engine origins (no meaningful
 *  channel — the engine is the channel). */
export function renderChannelSection(
  origin: MessageOrigin,
  fallbackParentTitle?: string,
  getLiveTitle?: (threadId: string) => string | undefined,
): preact.JSX.Element | null {
  switch (origin.kind) {
    case 'device':
      return (
        <div class="route-row">
          <strong>Device</strong>
          <span>{origin.label}</span>
        </div>
      );
    case 'api':
      return (
        <div class="route-row">
          <strong>API client</strong>
          <span>{origin.user_agent ?? '(no user-agent)'}</span>
        </div>
      );
    case 'workspace':
      return (
        <div class="route-row">
          <strong>Workspace</strong>
          <span>{origin.workspace}</span>
        </div>
      );
    case 'thread_link': {
      const title = resolveThreadLinkTitle(
        origin,
        fallbackParentTitle,
        getLiveTitle ?? (() => undefined),
      );
      const linkedId = origin.thread_id;
      const heading = origin.direction === 'child' ? 'Child thread' : 'Parent thread';
      return (
        <div class="route-row">
          <strong>{heading}</strong>
          <button
            type="button"
            class="accent-link"
            onClick={() => {
              focusThread(linkedId);
              closeMessageRoutePanel();
            }}
          >
            {title}
          </button>
        </div>
      );
    }
    case 'engine':
    case 'system':
      return null;
  }
}

/** Audit: cross-workspace thread/event IDs, parent spawning_event_id. Returns
 *  null when there's nothing extra to show beyond the channel. */
export function renderAuditSection(origin: MessageOrigin): preact.JSX.Element | null {
  switch (origin.kind) {
    case 'workspace':
      if (!origin.thread_id && !origin.event_id && !origin.user_agent) return null;
      return (
        <>
          {origin.thread_id && <div class="muted mono">thread: {origin.thread_id}</div>}
          {origin.event_id && <div class="muted mono">event: {origin.event_id}</div>}
          {origin.user_agent && <div class="muted">{origin.user_agent}</div>}
        </>
      );
    case 'thread_link':
      if (!origin.spawning_event_id) return null;
      return <div class="muted mono">event: {origin.spawning_event_id}</div>;
    case 'device':
    case 'api':
    case 'engine':
    case 'system':
      return null;
  }
}

/** Engine explainer: the "why" copy for engine-acted events. The heading is
 *  constant; the body comes from `describeEngineReason`. */
export function renderEngineExplainerSection(reason: EngineReason): preact.JSX.Element | null {
  const body = describeEngineReason(reason);
  if (!body) return null;
  return (
    <div class="route-explainer">
      <strong>Why the engine acted</strong>
      <p>{body}</p>
    </div>
  );
}

function renderExecutorSection(
  exchange: Exchange,
  threadEvents: Map<number, StoredEvent>,
  priorModel?: string,
  priorEffort?: string,
) {
  const model = exchangeResponseModel(exchange) ?? priorModel;
  const effort = exchangeReasoningEffort(exchange) ?? priorEffort;
  const extras = executorExtras(exchange, threadEvents);
  const hasContent = model || effort || extras.branch || extras.permissionMode
    || extras.ccSessionId || typeof extras.contextTokens === 'number';
  const ccActive = extras.branch !== undefined || extras.ccSessionId !== undefined;
  const repo = ccActive ? resolveRepoLabel(extras.repoId) : undefined;

  return (
    <section class="route-section">
      <h4>Executor</h4>
      {!hasContent && <div class="muted">No executor info yet</div>}
      {model && (
        <div class="route-row">
          <strong>Model</strong>
          <span>{displayModelName(model)}</span>
        </div>
      )}
      {effort && (
        <div class="route-row">
          <strong>Effort</strong>
          <span>{displayReasoningEffort(effort)}</span>
        </div>
      )}
      {typeof extras.contextTokens === 'number' && (
        <div class="route-row">
          <strong>Context</strong>
          <span>
            {extras.contextTokens.toLocaleString()} tokens
            {extras.contextTrimmed && <span class="pill"> trimmed</span>}
          </span>
        </div>
      )}
      {extras.permissionMode && (
        <div class="route-row">
          <strong>Permission</strong>
          <span>{extras.permissionMode}</span>
        </div>
      )}
      {repo && (
        <div class="route-row">
          <strong>Repository</strong>
          <span class={repo.failed ? 'error-text' : undefined}>{repo.text}</span>
        </div>
      )}
      {extras.branch && (
        <div class="route-row">
          <strong>Branch</strong>
          <span class="mono">{extras.branch}</span>
        </div>
      )}
      {extras.ccSessionId && (
        <div class="route-row">
          <strong>Session</strong>
          <span class="mono">{extras.ccSessionId}</span>
        </div>
      )}
    </section>
  );
}

/** CC repo label. Returns undefined for legacy events from before the engine
 *  always stamped `repo_id` — those sessions render no Repository row. */
function resolveRepoLabel(repoId: string | undefined): { text: string; failed?: boolean } | undefined {
  if (!repoId) return undefined;
  const repos = repositories.value;
  if (repos.status === 'not-loaded') loadRepositories();
  if (repos.status === 'failed') return { text: `${repoId} (load failed)`, failed: true };
  if (repos.status !== 'loaded') return { text: repoId };
  const match = repos.data.find(r => r.id === repoId);
  // The repo was registered when the session ran but has since been removed —
  // surface that instead of a bare UUID.
  return match ? { text: match.name } : { text: `${repoId} (deleted)` };
}

function renderTriggerOrigin(userEvent: Extract<StoredEvent, { type: 'TriggerStarted' }>) {
  const ts = triggers.value;
  const list = loadedOr(ts, []);
  // Fall back to name match when the stored trigger_id no longer resolves —
  // re-created triggers keep their name but get a new id, so events from the
  // old incarnation point at a dead id.
  const trigger = list.find(x => x.id === userEvent.trigger_id)
    ?? list.find(x => x.name === userEvent.trigger_name);
  const name = trigger?.name ?? userEvent.trigger_name ?? userEvent.trigger_id;
  const knownDeleted = ts.status === 'loaded' && !trigger;
  const invocation = userEvent.invocation;
  return (
    <div>
      <div class="route-row">
        <strong>Trigger</strong>
        {knownDeleted ? (
          <span>{name} <span class="muted">(deleted)</span></span>
        ) : (
          <button
            type="button"
            class="accent-link"
            onClick={() => {
              navigateToTrigger(trigger?.id ?? userEvent.trigger_id);
              closeMessageRoutePanel();
            }}
          >
            {name}
          </button>
        )}
      </div>
      {invocation?.kind === 'Event' && (
        <div class="route-row">
          <strong>Event</strong>
          <span class="mono">{invocation.event_type}</span>
        </div>
      )}
      {invocation?.kind === 'Event' && invocation.event_id && (
        <div class="muted mono">event: {invocation.event_id}</div>
      )}
    </div>
  );
}
