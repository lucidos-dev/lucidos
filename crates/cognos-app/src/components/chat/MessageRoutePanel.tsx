import { useRef } from 'preact/hooks';
import { messageRoutePanel, closeMessageRoutePanel, triggers, threadMap } from '../../store/store';
import { focusThread } from '../../store/actions/threads';
import { useDismissOnOutside, useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { loadedOr } from '../../store/types';
import {
  ENGINE_LABEL,
  exchangeResponseModel,
  exchangeReasoningEffort,
  displayModelName,
  displayReasoningEffort,
  isChangeLifecycleEvent,
  legacyOrigin,
  sortEventsChronologically,
  type Exchange,
  type EngineReason,
  type MessageOrigin,
  type StoredEvent,
} from '../../store/thread-events';

/** Exported for unit tests — see MessageRoutePanel.test.ts. */
export function resolveOrigin(userEvent: StoredEvent): MessageOrigin | undefined {
  if (userEvent.type === 'MessageReceived') return legacyOrigin(userEvent);
  if (isChangeLifecycleEvent(userEvent)) return userEvent.actor;
  // Engine-emitted events (SessionRecovered, CodingAgentPromptSent, TriggerStarted, ChangeProposed)
  // carry origin directly — surface it so the popover can render the Engine variant.
  if ('origin' in userEvent && userEvent.origin) {
    return userEvent.origin as MessageOrigin;
  }
  return undefined;
}

/** Exported for unit tests — see MessageRoutePanel.test.ts. */
export function engineReasonLabel(r: EngineReason): string {
  switch (r.kind) {
    case 'session_recovered': return 'Auto-resumed after restart';
    case 'orphan_recovery': return 'Orphan recovery';
    case 'scheduler': return r.trigger_name ? `Scheduled · ${r.trigger_name}` : 'Scheduled';
    case 'harden_retrigger': return 'Harden auto-retrigger';
    case 'stale_session': return 'Stale session cleanup';
  }
}

/** Exported for unit tests — see MessageRoutePanel.test.ts.
 *
 *  Branch + ccSessionId come from `SessionStarted` (or `SessionRecovered`), which fire
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
} {
  // The cutoff is the last seq in this exchange — SessionStarted typically lives in
  // `steps` right after the userEvent, so breaking at `userSeq` would exit too early.
  let lastSeq = exchange.userSeq;
  for (const { seq } of exchange.steps) {
    if (seq > lastSeq) lastSeq = seq;
  }

  let branch: string | undefined;
  let ccSessionId: string | undefined;
  for (const { seq, event } of sortEventsChronologically(threadEvents)) {
    if (event.type === 'SessionStarted') {
      if (event.branch) branch = event.branch;
      if (event.session_id) ccSessionId = event.session_id;
    } else if (event.type === 'SessionRecovered' && event.branch) {
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
      if (typeof event.context_tokens === 'number') contextTokens = event.context_tokens;
      if (typeof event.trimmed === 'boolean') contextTrimmed = event.trimmed;
    }
  }
  return { branch, permissionMode, ccSessionId, contextTokens, contextTrimmed };
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
  const userEvent = exchange.userEvent;
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
        ? renderOriginSection(userEvent, thread.meta.parentThreadTitle)
        : renderExecutorSection(exchange, thread.events, priorModel, priorEffort)}
    </div>
  );
}

function renderOriginSection(userEvent: StoredEvent, parentTitle: string | undefined) {
  return (
    <section class="route-section">
      <h4>Origin</h4>
      {renderOrigin(userEvent, resolveOrigin(userEvent), parentTitle)}
    </section>
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

function renderOrigin(
  userEvent: StoredEvent,
  origin: MessageOrigin | undefined,
  fallbackParentTitle: string | undefined,
) {
  if (userEvent.type === 'TriggerStarted') {
    // TriggerStarted has its own renderer that knows about invocation kind
    // (Schedule vs Event) and the source event_id — richer than the generic
    // engine origin's `Scheduled · NAME` label.
    return renderTriggerOrigin(userEvent);
  }
  // MissingHardeningDetected and MergeConflictDetected don't yet carry an engine origin —
  // keep the hardcoded labels until they're migrated.
  if (userEvent.type === 'MissingHardeningDetected') {
    return (
      <div>
        <strong>{ENGINE_LABEL}</strong> · auto-hardening required
      </div>
    );
  }
  if (userEvent.type === 'MergeConflictDetected') {
    return (
      <div>
        <strong>{ENGINE_LABEL}</strong> · resolving merge conflict from main
      </div>
    );
  }
  if (!origin) return <div class="muted">Unknown</div>;
  switch (origin.kind) {
    case 'device':
      // Device-attribution = the human user on this device. Render with the
      // same `\u{1F464}` icon + "You" label that the chat exchange header uses
      // for user MessageReceived events (see ChatExchange.tsx::initiatorFor),
      // so device-attributed actions look like the user's other inputs.
      return (
        <div class="route-row">
          <span class="initiator-icon">{'\u{1F464}'}</span>
          <strong>You</strong>
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
        <div>
          <div class="route-row">
            <strong>Workspace</strong>
            <span>{origin.workspace}</span>
            {origin.mode && origin.mode !== 'human' && (
              <span class={`pill mode-${origin.mode}`}>{origin.mode}</span>
            )}
          </div>
          {origin.thread_id && <div class="muted mono">thread: {origin.thread_id}</div>}
          {origin.event_id && <div class="muted mono">event: {origin.event_id}</div>}
          {origin.user_agent && <div class="muted">{origin.user_agent}</div>}
        </div>
      );
    case 'parent_thread': {
      const title = origin.title ?? fallbackParentTitle ?? origin.thread_id;
      const parentId = origin.thread_id;
      const mode = origin.mode;
      return (
        <div class="route-row">
          <strong>Parent thread</strong>
          <a
            href="#"
            onClick={(e: MouseEvent) => {
              e.preventDefault();
              focusThread(parentId);
              closeMessageRoutePanel();
            }}
          >
            {title}
          </a>
          {mode && mode !== 'human' && (
            <span class={`pill mode-${mode}`}>{mode}</span>
          )}
        </div>
      );
    }
    case 'engine':
      return renderEngineOrigin(origin.reason);
  }
}

function renderEngineOrigin(reason: EngineReason) {
  return (
    <div class="route-row">
      <strong>Engine</strong>
      <span>{engineReasonLabel(reason)}</span>
    </div>
  );
}

/**
 * Render the origin section for a TriggerStarted event.
 *
 * Label rules:
 * - `invocation.kind === 'Schedule'` → "Scheduled"
 * - `invocation.kind === 'Event'`    → "Event triggered" + the matched event_type
 * - missing invocation (legacy rows) → "Trigger"
 *
 * For the Event case we also surface the source event_id so the popover
 * deep-links back to the event row that fired the run.
 */
function renderTriggerOrigin(userEvent: Extract<StoredEvent, { type: 'TriggerStarted' }>) {
  const ts = loadedOr(triggers.value, []);
  const trigger = ts.find(x => x.id === userEvent.trigger_id);
  const name = trigger?.name ?? userEvent.trigger_name ?? userEvent.trigger_id;
  const invocation = userEvent.invocation;
  const label =
    invocation?.kind === 'Event' ? 'Event triggered'
    : invocation?.kind === 'Schedule' ? 'Scheduled'
    : 'Trigger';
  return (
    <div>
      <div class="route-row">
        <strong>{label}</strong>
        <span>{name}</span>
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
