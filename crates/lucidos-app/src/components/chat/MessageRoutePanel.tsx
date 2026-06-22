import { useRef } from 'preact/hooks';
import { messageRoutePanel, closeMessageRoutePanel, triggers, threadMap, repositories, appsList, workspaceName } from '../../store/store';
import { loadApps } from '../../store/actions/apps';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import {
  openThreadAcrossWorkspaces,
  crossWorkspaceThreadTitle,
  ensureCrossWorkspaceThreadTitle,
} from '../../store/actions/cross-workspace';
import { navigateToTrigger } from '../../store/actions/triggers';
import { loadRepositories } from '../../store/actions/chat';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { Overlay } from '../shared/Overlay';
import { loadedOr } from '../../store/types';
import {
  exchangeResponseModel,
  exchangeReasoningEffort,
  displayModelName,
  displayReasoningEffort,
  findCommandPermissionResolution,
  findMcpPermissionResolution,
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
  type ThreadMeta,
} from '../../store/thread-events';
import { describeAbortCause, describeCancelCause, describeEngineReason } from '../../utils/engineEventExplainers';

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
  if (userEvent.type === 'CommandPermissionRequested') {
    return findCommandPermissionResolution(exchange, userEvent.request_id)?.actor;
  }
  if (userEvent.type === 'McpPermissionRequested') {
    return findMcpPermissionResolution(exchange, userEvent.request_id)?.actor;
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

/** Branch comes from `SessionStarted` (or `ContinuationStarted`), which fire once per CC
 *  process spawn — not per user message. A follow-up exchange within an existing Claude Code
 *  session has no SessionStarted in its own steps, so we walk the full thread up to this
 *  exchange's user event and track the most recent branch-defining event.
 *
 *  The session id is NOT on `SessionStarted` — the engine emits that with an empty
 *  `session_id` at spawn time (the real id isn't known yet). The agent reports it in its
 *  `Init` event, which the engine persists onto `CodingAgentSettingsChanged.cc_session_id`
 *  (and later `CodingAgentIdled.cc_session_id`) for both Claude Code and Codex. So we read
 *  the session id from those, falling back to a non-empty `SessionStarted.session_id` only
 *  for legacy rows. Like branch, it's session-scoped, so it rides the same full-thread walk.
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
      // session_id is empty on real SessionStarted rows (see doc comment) — only
      // legacy rows ever carry it here; the live id lands on the events below.
      if (event.session_id) ccSessionId = event.session_id;
      // Each SessionStarted snapshots its own repo bind — don't fall back to a
      // prior session's repo_id when the current SessionStarted lacks one
      // (legacy events from before the engine always stamped repo_id).
      repoId = event.repo_id;
    } else if (event.type === 'ContinuationStarted' && event.branch) {
      branch = event.branch;
    } else if (event.type === 'CodingAgentSettingsChanged' || event.type === 'CodingAgentIdled') {
      // The agent's Init session id is persisted onto CodingAgentSettingsChanged
      // (and CodingAgentIdled), for both Claude Code and Codex — this is the real
      // source for the Session row.
      if (event.cc_session_id) ccSessionId = event.cc_session_id;
    }
    if (seq === lastSeq) break;
  }

  let permissionMode: string | undefined;
  let contextTokens: number | undefined;
  let contextTrimmed: boolean | undefined;
  for (const { event } of exchange.steps) {
    if (event.type === 'CodingAgentSettingsChanged') {
      if (event.permission_mode) permissionMode = event.permission_mode;
    } else if (event.type === 'ThoughtStreamed') {
      // Legacy DB rows — ContextCaptured below overrides when present.
      if (typeof event.context_tokens === 'number') contextTokens = event.context_tokens;
      if (typeof event.trimmed === 'boolean') contextTrimmed = event.trimmed;
    } else if (event.type === 'ContextCaptured') {
      // Prefer real input_tokens; fall back to the conservative chars*2/3
      // estimate (see `estimate_tokens_from_chars` in the engine).
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

  if (!state) return null;
  const { exchange, threadId, section, priorModel, priorEffort } = state;
  const thread = threadMap.value.get(threadId);
  if (!thread) return null;

  return (
    <Overlay
      open
      onClose={closeMessageRoutePanel}
      anchor={state.anchor}
      backdrop={false}
      panelClass={`message-route-panel ${pos?.placement ?? ''}`}
      panelStyle={pos ? { top: `${pos.top}px`, left: `${pos.left}px` } : { visibility: 'hidden' }}
      panelRole="dialog"
      panelProps={{ 'aria-label': section === 'origin' ? 'Initiator info' : 'Executor info' }}
      panelRef={ref}
    >
      {section === 'origin'
        ? renderOriginSection(exchange, thread.meta.parentThreadTitle, getLiveThreadTitle)
        : renderExecutorSection(exchange, thread.events, thread.meta, priorModel, priorEffort)}
    </Overlay>
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

  // System-driven `ResponseAborted` (safety_net, engine_shutdown, …): the
  // device/api/v1/workspace/engine renderers above all return null for
  // `kind: 'system'`, so without this branch the panel falls through to the
  // bare "Unknown" fallback below — even though the event carries a typed
  // `cause`. Skip when there's a device actor (e.g. /api/v1/restart) so the
  // existing "You — Restarted" path keeps rendering.
  if (
    userEvent.type === 'ResponseAborted'
    && (origin === undefined || origin.kind === 'system')
  ) {
    return (
      <section class="route-section">
        <h4>Origin</h4>
        <div class="route-row">
          <strong>Issued by</strong>
          <span>System</span>
        </div>
        <div class="route-explainer">
          <strong>Why the response stopped</strong>
          <p>{describeAbortCause(userEvent.cause)}</p>
        </div>
      </section>
    );
  }

  // User-driven `ResponseCanceled` (CancelCause). The "Response canceled" turn
  // carries no actor chip — this popover IS its initiator disclosure. Cancel is
  // user-driven by definition, so always attribute it to "You"; include the
  // device when one was recorded (the chat-thread cancel path doesn't plumb an
  // actor, so it's often absent). The cause line explains how it stopped.
  if (userEvent.type === 'ResponseCanceled') {
    return (
      <section class="route-section">
        <h4>Origin</h4>
        <div class="route-row">
          <strong>Issued by</strong>
          <span>You</span>
        </div>
        {origin?.kind === 'device' && renderChannelSection(origin)}
        <div class="route-explainer">
          <strong>Why the response stopped</strong>
          <p>{describeCancelCause(userEvent.cause)}</p>
        </div>
      </section>
    );
  }

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

/** Initiator row for divider-starter ActionRequired events. The chip reads the
 *  asking agent (Lucidos Agent / Claude Code); this row mirrors who *asked* —
 *  Claude Code or Lucidos Agent for questions (by thread kind), Claude Code for
 *  permission gates, Lucidos for credential and MCP-consent requests. Returns
 *  null for non-divider events (their initiator is implied by channel/audit). */
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
    case 'UserQuestionAsked':            return userEvent.cc_session_id ? 'Claude Code' : 'Lucidos Agent';
    case 'CodingAgentPermissionRequest': return 'Claude Code (permission gate)';
    case 'CommandPermissionRequested':   return 'Lucidos Agent (command gate)';
    case 'McpPermissionRequested':       return 'Lucidos Agent (MCP gate)';
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
    case 'api': {
      // `source_thread_id` is set when the engine recognised this request
      // as coming from a Lucidos-spawned subprocess (CC, run_bash,
      // run_python, scheduled script, `lucidos` CLI). Surface it as a
      // deep-link so the popover answers "which agent did this".
      const sourceThreadId = origin.source_thread_id;
      const sourceTitle = sourceThreadId
        ? getLiveTitle?.(sourceThreadId) ?? `thread ${sourceThreadId.slice(0, 8)}`
        : null;
      return (
        <div class="route-row">
          <strong>API client</strong>
          <span>{origin.user_agent ?? '(no user-agent)'}</span>
          {sourceThreadId && (
            <button
              type="button"
              class="accent-link"
              onClick={() => {
                focusThreadOrBootstrap(sourceThreadId);
                closeMessageRoutePanel();
              }}
            >
              {sourceTitle ?? 'spawning thread'}
            </button>
          )}
        </div>
      );
    }
    case 'workspace': {
      const tid = origin.thread_id;
      return (
        <>
          <div class="route-row">
            <strong>Workspace</strong>
            <span>{origin.workspace}</span>
          </div>
          {tid && renderWorkspaceThreadLink(origin.workspace, tid, getLiveTitle)}
        </>
      );
    }
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
              focusThreadOrBootstrap(linkedId);
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

/** Named, clickable link to a Workspace-origin's source thread. Resolves the
 *  title: same-workspace → live `threadMap` lookup; cross-workspace → the
 *  best-effort live-fetch cache (firing the fetch when absent, which re-renders
 *  the popover once it arrives). Falls back to a short id so the link is never
 *  blank. The click routes cross-workspace-aware — focus in place when local,
 *  hop to the source workspace's UI when remote. */
function renderWorkspaceThreadLink(
  workspace: string,
  threadId: string,
  getLiveTitle?: (threadId: string) => string | undefined,
): preact.JSX.Element {
  const isLocal = !workspaceName.value || workspace === workspaceName.value;
  let title: string | undefined;
  if (isLocal) {
    title = getLiveTitle?.(threadId);
  } else {
    title = crossWorkspaceThreadTitle(workspace, threadId);
    if (title === undefined) void ensureCrossWorkspaceThreadTitle(workspace, threadId);
  }
  const label =
    title && title !== PENDING_TITLE_PLACEHOLDER ? title : `thread ${threadId.slice(0, 8)}`;
  return (
    <div class="route-row">
      <strong>Thread</strong>
      <button
        type="button"
        class="accent-link"
        onClick={() => {
          openThreadAcrossWorkspaces(workspace, threadId);
          closeMessageRoutePanel();
        }}
      >
        {label}
      </button>
    </div>
  );
}

/** Audit: cross-workspace event id, parent spawning_event_id, raw user-agent.
 *  Returns null when there's nothing extra to show beyond the channel. */
export function renderAuditSection(origin: MessageOrigin): preact.JSX.Element | null {
  switch (origin.kind) {
    case 'workspace':
      // The thread id is rendered as a named link in the channel section
      // (`renderWorkspaceThreadLink`); the audit section keeps only the
      // spawning event id and the raw user-agent.
      if (!origin.event_id && !origin.user_agent) return null;
      return (
        <>
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
  meta: ThreadMeta,
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
  // App coding-agent threads carry their kind on the projection; resolve the
  // app's icon + display name so the Branch row reads "{icon} {name} ·
  // claude-code/app/<id>/..." at a glance. The folder's last segment is the
  // canonical app id.
  const appInfo = meta.codingAgentKind === 'app' ? resolveAppInfo(meta.codingAgentFolder) : undefined;

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
          <span class="route-branch">
            {appInfo && (
              <>
                <span class="route-branch-app-icon" aria-hidden="true">{appInfo.icon}</span>
                <span class={appInfo.failed ? 'route-branch-app-name error-text' : 'route-branch-app-name'}>{appInfo.name}</span>
                <span class="route-branch-sep" aria-hidden="true">·</span>
              </>
            )}
            <span class="mono">{extras.branch}</span>
          </span>
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

/** App icon + name for the Branch row prefix on app coding-agent threads.
 *  Distinguishes the four `Loadable` states so a failed appsList fetch
 *  doesn't render identically to a loading one (per frontend.md). */
function resolveAppInfo(
  folder: string | undefined,
): { icon: string; name: string; failed?: boolean } | undefined {
  if (!folder) return undefined;
  const appId = folder.split('/').filter(Boolean).pop();
  if (!appId) return undefined;
  const apps = appsList.value;
  // Idempotent kick-off from the render path: loadApps() flips the signal to
  // `loading` synchronously (setLoadingIfFresh), so the next render no longer
  // sees `not-loaded` and won't re-fetch — exactly one fetch fires.
  if (apps.status === 'not-loaded') void loadApps();
  const FALLBACK_ICON = '\u{1F4E6}'; // 📦 — neutral "app" pictogram
  if (apps.status === 'failed') {
    return { icon: FALLBACK_ICON, name: `${appId} (apps failed to load)`, failed: true };
  }
  if (apps.status !== 'loaded') {
    return { icon: FALLBACK_ICON, name: appId };
  }
  const match = apps.data.find(a => a.id === appId);
  return {
    icon: match?.icon || FALLBACK_ICON,
    name: match?.name || appId,
  };
}

/** CC repo label. Returns undefined for legacy events from before the engine
 *  always stamped `repo_id` — those sessions render no Repository row. */
function resolveRepoLabel(repoId: string | undefined): { text: string; failed?: boolean } | undefined {
  if (!repoId) return undefined;
  const repos = repositories.value;
  // Idempotent render-path kick-off — see resolveAppInfo above: loadRepositories
  // flips the signal to `loading` synchronously, so only one fetch fires.
  if (repos.status === 'not-loaded') void loadRepositories();
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
              void navigateToTrigger(trigger?.id ?? userEvent.trigger_id);
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
