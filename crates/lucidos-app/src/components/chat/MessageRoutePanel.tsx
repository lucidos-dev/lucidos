import { openSettingsSubview } from '../../store/actions/menu';
import { useRef } from 'preact/hooks';
import { UNTITLED_THREAD } from '../../utils/threadTitle';
import type { ComponentChildren } from 'preact';
import { AppIcon } from '../shared/AppIcon';
import { messageRoutePanel, closeMessageRoutePanel, contextViewer, triggers, threadMap, repositories, appsList, workspaceName, settingsScrollTarget } from '../../store/store';
import { loadApps } from '../../store/actions/apps';
import { turnDeviceName } from '../../store/actions/devices';
import { gatewayUsers, userName } from '../../store/actions/gatewayUsers';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import {
  openThreadAcrossWorkspaces,
  crossWorkspaceThreadTitle,
  ensureCrossWorkspaceThreadTitle,
} from '../../store/actions/cross-workspace';
import { navigateToTrigger } from '../../store/actions/triggers';
import { loadRepositories } from '../../store/actions/chat';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { Overlay } from '../shared/Overlay';
import { EventRowFoldView } from './EventRow';
import { loadedOr } from '../../store/types';
import {
  capturedEventToData,
  ENGINE_LABEL,
  LUCIDOS_AGENT_LABEL,
  exchangeResponseModel,
  exchangeReasoningEffort,
  displayModelName,
  displayReasoningEffort,
  modeToInitiator,
  PENDING_TITLE_PLACEHOLDER,
  plainEventName,
  sortEventsChronologically,
  SYSTEM_LABEL,
  unattestedCallerLabel,
  type Exchange,
  type EngineReason,
  type MessageOrigin,
  type SpawnVerdict,
  type StoredEvent,
  type ThreadMeta,
} from '../../store/thread-events';
import type { ContextCapture } from '../../store/types';
import {
  describeAbortCause,
  describeCancelCause,
  describeContinuationReason,
  describeEngineReason,
  engineReasonConfirmingDevice,
  engineReasonHeadline,
  UNRECORDED_ENGINE_SEED_EXPLAINER,
} from '../../utils/engineEventExplainers';
import { dividerSettlement, resolveOrigin, type DividerSettlement } from './messageOrigin';

export function resolveThreadLinkTitle(
  origin: Extract<MessageOrigin, { kind: 'thread_link' }>,
  cachedLinkedTitle: string | undefined,
  getLiveTitle: (threadId: string) => string | undefined,
): string {
  const live = getLiveTitle(origin.thread_id);
  if (live && live !== PENDING_TITLE_PLACEHOLDER) return live;
  if (origin.title) return origin.title;
  if (cachedLinkedTitle) return cachedLinkedTitle;
  return UNTITLED_THREAD;
}


export function executorExtras(
  exchange: Exchange,
  threadEvents: Map<number, StoredEvent>,
): {
  branch?: string;
  ccSessionId?: string;
  contextTokens?: number;
  contextTrimmed?: boolean;
  contextCapture?: ContextCapture;
  repoId?: string;
} {
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
      repoId = event.repo_id;
    } else if (event.type === 'ContinuationStarted' && event.branch) {
      branch = event.branch;
    } else if (event.type === 'CodingAgentSettingsChanged' || event.type === 'CodingAgentIdled') {
      if (event.cc_session_id) ccSessionId = event.cc_session_id;
    }
    if (seq === lastSeq) break;
  }

  let contextTokens: number | undefined;
  let contextTrimmed: boolean | undefined;
  let contextCapture: ContextCapture | undefined;
  for (const { event } of exchange.steps) {
    if (event.type === 'ThoughtStreamed') {
      if (typeof event.context_tokens === 'number') {
        contextTokens = event.context_tokens;
        contextCapture = undefined;
      }
      if (typeof event.trimmed === 'boolean') contextTrimmed = event.trimmed;
    } else if (event.type === 'ContextCaptured') {
      contextCapture = capturedEventToData(event, event._eventId);
      const tokens = event.usage?.input_tokens ?? event.estimated_total_tokens;
      if (typeof tokens === 'number') contextTokens = tokens;
      if (typeof event.trimmed === 'boolean') contextTrimmed = event.trimmed;
    }
  }
  return { branch, ccSessionId, contextTokens, contextTrimmed, contextCapture, repoId };
}

export function MessageRoutePanel() {
  const state = messageRoutePanel.value;
  const ref = useRef<HTMLDivElement>(null);
  const pos = useAnchoredPosition(state?.anchor ?? null, ref, '.thread-pane');

  if (!state) return null;
  const { exchange, threadId, section, priorModel, priorEffort, pausedBy, readAt } = state;
  const thread = threadMap.value.get(threadId);
  if (!thread) return null;

  return (
    <Overlay
      open
      onClose={closeMessageRoutePanel}
      anchor={state.anchor}
      backdrop={false}
      panelClass={`surface message-route-panel ${pos?.placement ?? ''}`}
      panelStyle={pos ? { top: `${pos.top}px`, left: `${pos.left}px` } : { visibility: 'hidden' }}
      panelRole="dialog"
      panelProps={{ 'aria-label': section === 'origin' ? 'Who started this' : 'Who answered' }}
      panelRef={ref}
    >
      {section === 'origin'
        ? renderOriginSection(exchange, thread.meta.parentThreadTitle, getLiveThreadTitle, pausedBy, readAt)
        : renderExecutorSection(exchange, thread.events, thread.meta, priorModel, priorEffort)}
    </Overlay>
  );
}

function getLiveThreadTitle(threadId: string): string | undefined {
  return threadMap.value.get(threadId)?.meta.title;
}

export function renderOriginSection(
  exchange: Exchange,
  parentTitle: string | undefined,
  getLiveTitle: (threadId: string) => string | undefined,
  pausedBy?: StoredEvent,
  readAt?: string,
) {
  const userEvent = exchange.userEvent;
  if (userEvent.type === 'TriggerStarted') {
    return (
      <section class="route-section">
        <h4>Origin</h4>
        {renderTriggerOrigin(userEvent)}
        {renderTechnicalDetails(userEvent, undefined)}
      </section>
    );
  }

  const origin = resolveOrigin(exchange);
  const settledAt = renderSettledAt(userEvent, dividerSettlement(exchange));
  const technical = renderTechnicalDetails(userEvent, origin);
  const channel = origin
    ? renderChannelSection(origin, parentTitle, getLiveTitle, deviceRowLabel(userEvent))
    : null;
  const unrecordedSeed = !origin && isUnrecordedEngineSeed(userEvent);
  const issuer = origin?.kind === 'engine' || unrecordedSeed ? renderIssuedByRow(ENGINE_LABEL)
    : origin?.kind === 'system' ? renderIssuedByRow(SYSTEM_LABEL)
      : origin?.kind === 'agent' ? renderIssuedByRow(LUCIDOS_AGENT_LABEL)
        : null;
  const engineRows = origin?.kind === 'engine' ? renderEngineReasonRows(origin.reason) : null;
  const continuationWhy = userEvent.type === 'ContinuationStarted'
    ? renderExplainer('Why this resumed', describeContinuationReason(userEvent.reason))
    : null;
  const explainer = continuationWhy
    ?? (origin?.kind === 'engine' ? renderEngineExplainerSection(origin.reason)
      : origin?.kind === 'unattested_caller' ? renderCallerExplainer(origin)
      : unrecordedSeed ? renderExplainer(ENGINE_WHY_HEADING, UNRECORDED_ENGINE_SEED_EXPLAINER)
        : null);
  const paused = pausedBy ? renderFoldedPause(pausedBy) : null;
  const delivery = readAt ? renderDelivery(userEvent, readAt) : null;

  if (userEvent.type === 'ResponseAborted') {
    const byHost = origin === undefined || origin.kind === 'system';
    return (
      <section class="route-section">
        <h4>Origin</h4>
        {byHost ? renderIssuedByRow(SYSTEM_LABEL) : channel}
        {renderExplainer('Why the response stopped', describeAbortCause(userEvent.cause))}
        {technical}
      </section>
    );
  }

  if (userEvent.type === 'ResponseCanceled') {
    return (
      <section class="route-section">
        <h4>Origin</h4>
        {renderIssuedByRow('You')}
        {origin?.kind === 'device' && channel}
        {renderExplainer('Why the response stopped', describeCancelCause(userEvent.cause))}
        {technical}
      </section>
    );
  }

  if (!issuer && !channel && !explainer && !paused) {
    return (
      <section class="route-section">
        <h4>Origin</h4>
        <div class="route-row">
          <strong>Sent by</strong>
          <span class="muted">Not recorded</span>
        </div>
        {renderExplainer('Why this is blank', NOT_RECORDED_EXPLAINER)}
        {delivery}
        {technical}
      </section>
    );
  }

  return (
    <section class="route-section">
      <h4>Origin</h4>
      {settledAt}
      {issuer}
      {engineRows}
      {channel}
      {delivery}
      {explainer}
      {paused}
      {technical}
    </section>
  );
}

const ENGINE_WHY_HEADING = 'Why Lucidos acted';

const NOT_RECORDED_EXPLAINER =
  'Lucidos did not record who started this turn. Older turns often lack it.';

function settledVerb(userEvent: StoredEvent): 'Answered' | 'Decided' | null {
  switch (userEvent.type) {
    case 'UserQuestionAsked':            return 'Answered';
    case 'CodingAgentPermissionRequest':
    case 'CommandPermissionRequested':
    case 'McpPermissionRequested':       return 'Decided';
    default:                             return null;
  }
}

function deviceRowLabel(userEvent: StoredEvent): string {
  const verb = settledVerb(userEvent);
  if (verb) return `${verb} on`;
  switch (userEvent.type) {
    case 'MessageReceived':
    case 'PromptInjected':               return 'Sent from';
    case 'ResponseAborted':
    case 'ResponseCanceled':
    case 'EventWaitCanceled':            return 'Stopped from';
    default:                             return 'Device';
  }
}

function renderTechnicalDetails(
  userEvent: StoredEvent,
  origin: MessageOrigin | undefined,
): preact.JSX.Element {
  return (
    <div class="route-technical" data-role="route-technical">
      <EventRowFoldView
        label="Technical details"
        body={technicalDetailRows(userEvent, origin).map(([label, value]) => (
          <div key={label} class="route-row">
            <strong>{label}</strong>
            <span class="mono">{value}</span>
          </div>
        ))}
      />
    </div>
  );
}

function spawnVerdictText(verdict: SpawnVerdict): string {
  switch (verdict.kind) {
    case 'allowed':    return 'Allowed';
    case 'unattested': return 'The calling workspace could not establish who started its thread';
    case 'refused':    return `Refused: ${verdict.reason}`;
  }
}

export function technicalDetailRows(
  userEvent: StoredEvent,
  origin: MessageOrigin | undefined,
): [string, string][] {
  const rows: [string, string][] = [['Event type', userEvent.type]];
  if (origin?.kind === 'api' && origin.user_agent) rows.push(['Client', origin.user_agent]);
  if (origin?.kind === 'unattested_caller') {
    if (origin.claimed_device_id) rows.push(['Claimed device id', origin.claimed_device_id]);
    if (origin.claimed_local_token) rows.push(['Claimed credential', 'Machine-local token']);
    if (origin.claimed_workspace) rows.push(['Claimed workspace', origin.claimed_workspace]);
    if (origin.claimed_thread_id) rows.push(['Claimed thread id', origin.claimed_thread_id]);
    if (origin.user_agent) rows.push(['Client', origin.user_agent]);
  }
  if (origin?.kind === 'workspace') {
    if (origin.thread_id) rows.push(['Thread id', origin.thread_id]);
    if (origin.event_id) rows.push(['Event id', origin.event_id]);
    if (origin.user_agent) rows.push(['Client', origin.user_agent]);
    if (origin.verdict) rows.push(['Its gate said', spawnVerdictText(origin.verdict)]);
  }
  if (origin?.kind === 'engine' && origin.reason.kind === 'event_wait' && origin.reason.watched.length > 0) {
    rows.push(['Waited for', origin.reason.watched.join(', ')]);
  }
  if (origin?.kind === 'thread_link') {
    rows.push(['Thread id', origin.thread_id]);
    if (origin.spawning_event_id) rows.push(['Event id', origin.spawning_event_id]);
  }
  if (userEvent.type === 'TriggerStarted' && userEvent.invocation?.kind === 'Event') {
    rows.push(['Fired on', userEvent.invocation.event_type]);
    if (userEvent.invocation.event_id) rows.push(['Event id', userEvent.invocation.event_id]);
  }
  return rows;
}

function renderDelivery(message: StoredEvent, readAt: string): preact.JSX.Element {
  const sentAt = message.created || message._displayCreated;
  return (
    <>
      {sentAt && (
        <div class="route-row">
          <strong>Sent</strong>
          <span>{formatMessageTimestamp(sentAt)}</span>
        </div>
      )}
      <div class="route-row">
        <strong>Read</strong>
        <span>{formatMessageTimestamp(readAt)}</span>
      </div>
    </>
  );
}

function renderFoldedPause(pause: StoredEvent): preact.JSX.Element {
  const { cause, actor } = pause.type === 'ResponseAborted' ? pause : { cause: undefined, actor: undefined };
  const pausedAt = pause.created || pause._displayCreated;
  return (
    <>
      {pausedAt && (
        <div class="route-row">
          <strong>Paused</strong>
          <span>{formatMessageTimestamp(pausedAt)}</span>
        </div>
      )}
      {renderExplainer('Why it paused', describeAbortCause(cause))}
      {actor?.kind === 'device' && (
        <div class="route-row">
          <strong>Restarted from</strong>
          <span>{turnDeviceName(actor.device_id)}</span>
        </div>
      )}
    </>
  );
}

function isUnrecordedEngineSeed(userEvent: StoredEvent): boolean {
  switch (userEvent.type) {
    case 'MessageReceived':
    case 'PromptInjected':
      return modeToInitiator(userEvent.mode) === 'system';
    case 'CodingAgentPromptSent':
      return true;
    default:
      return false;
  }
}

function renderEngineReasonRows(reason: EngineReason): preact.JSX.Element | null {
  const headline = engineReasonHeadline(reason);
  const deviceId = engineReasonConfirmingDevice(reason);
  if (!headline && !deviceId) return null;
  return (
    <>
      {headline && (
        <div class="route-row">
          <strong>{headline.label}</strong>
          <span>{headline.value}</span>
        </div>
      )}
      {deviceId && (
        <div class="route-row">
          <strong>Confirmed on</strong>
          <span>{turnDeviceName(deviceId)}</span>
        </div>
      )}
    </>
  );
}

function renderIssuedByRow(label: string): preact.JSX.Element {
  return (
    <div class="route-row">
      <strong>Issued by</strong>
      <span>{label}</span>
    </div>
  );
}

function renderCallerExplainer(origin: Extract<MessageOrigin, { kind: 'unattested_caller' }>) {
  if (!origin.claimed_workspace) return renderExplainer('Who sent this', origin.claimed_device_id
    ? 'It named a device, but nothing checked that claim, so Lucidos cannot tell which program sent it.'
    : origin.claimed_local_token
      ? 'It reached Lucidos from this computer but named no workspace or thread, so Lucidos cannot tell which program it was.'
      : 'This request named no workspace, device or program, so Lucidos cannot tell who sent it.');
  return (
    <div class="route-explainer">
      <strong>How Lucidos can check it</strong>
      <p>{`This request reached ${workspaceName.value} directly, not through a gateway, so nothing checked who sent it. Lucidos can check calls sent by a thread through its gateway. If the workspaces use separate gateways, pair them first.`}</p>
      <button class="accent-link" onClick={() => {
        openSettingsSubview('access');
        settingsScrollTarget.value = 'access:pair-gateway';
        closeMessageRoutePanel();
      }}>Pair a gateway in Settings</button>
    </div>
  );
}

function renderExplainer(heading: string, body: string | null): preact.JSX.Element | null {
  if (!body) return null;
  return (
    <div class="route-explainer">
      <strong>{heading}</strong>
      <p>{body}</p>
    </div>
  );
}

function renderSettledAt(
  userEvent: StoredEvent,
  settlement: DividerSettlement | undefined | null,
): preact.JSX.Element | null {
  const verb = settledVerb(userEvent);
  const at = settlement && (settlement.created || settlement._displayCreated);
  if (!verb || !at) return null;
  return (
    <div class="route-row">
      <strong>{verb}</strong>
      <span>{formatMessageTimestamp(at)}</span>
    </div>
  );
}

export function renderChannelSection(
  origin: MessageOrigin,
  fallbackParentTitle?: string,
  getLiveTitle?: (threadId: string) => string | undefined,
  deviceLabel = 'Device',
): preact.JSX.Element | null {
  switch (origin.kind) {
    case 'saved_draft': {
      const threadId = origin.submitted_by_thread_id;
      return (
        <>
          <div class="route-row"><strong>Content</strong><span>Your saved draft</span></div>
          <div class="route-row">
            <strong>Submitted by</strong>
            <button type="button" class="accent-link" onClick={() => {
              focusThreadOrBootstrap(threadId);
              closeMessageRoutePanel();
            }}>{getLiveTitle?.(threadId) ?? UNTITLED_THREAD}</button>
          </div>
        </>
      );
    }
    case 'device': {
      const user = userName(gatewayUsers.value, origin.device_id);
      return (
        <>
          {user && (
            <div class="route-row">
              <strong>User</strong>
              <span>{user}</span>
            </div>
          )}
          <div class="route-row">
            <strong>{deviceLabel}</strong>
            <span>{turnDeviceName(origin.device_id)}</span>
          </div>
        </>
      );
    }
    case 'api': {
      const sourceThreadId = origin.source_thread_id;
      const sourceTitle = sourceThreadId
        ? getLiveTitle?.(sourceThreadId) ?? UNTITLED_THREAD
        : null;
      return (
        <div class="route-row">
          <strong>Sent by</strong>
          <span class="route-value-group">
            <span>{sourceThreadId ? 'A script run by the agent in' : 'An app or script outside Lucidos'}</span>
            {sourceThreadId && (
              <button
                type="button"
                class="accent-link"
                onClick={() => {
                  focusThreadOrBootstrap(sourceThreadId);
                  closeMessageRoutePanel();
                }}
              >
                {sourceTitle}
              </button>
            )}
          </span>
        </div>
      );
    }
    case 'unattested_caller':
      return (
        <div class="route-row">
          <strong>Sent from</strong>
          <span>{unattestedCallerLabel(origin, origin.claimed_device_id ? turnDeviceName(origin.claimed_device_id, { knownOnly: true }) : undefined)}{origin.claimed_workspace ? ', by its own account' : ''}</span>
        </div>
      );
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
      const heading = origin.direction === 'child' ? 'Child thread'
        : threadMap.value.get(linkedId)?.meta.home ? 'Home thread'
        : 'Parent thread';
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
    case 'webhook':
      return (
        <div class="route-row">
          <strong>Webhook</strong>
          <span>{origin.name}</span>
        </div>
      );
    case 'agent':
    case 'engine':
    case 'system':
      return null;
  }
}

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
    title && title !== PENDING_TITLE_PLACEHOLDER ? title : UNTITLED_THREAD;
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

export function renderEngineExplainerSection(reason: EngineReason): preact.JSX.Element | null {
  return renderExplainer(ENGINE_WHY_HEADING, describeEngineReason(reason));
}

export function renderExecutorSection(
  exchange: Exchange,
  threadEvents: Map<number, StoredEvent>,
  meta: ThreadMeta,
  priorModel?: string,
  priorEffort?: string,
) {
  const model = exchangeResponseModel(exchange) ?? priorModel;
  const effort = exchangeReasoningEffort(exchange) ?? priorEffort;
  const extras = executorExtras(exchange, threadEvents);
  const hasContent = model || effort || extras.branch
    || extras.ccSessionId || typeof extras.contextTokens === 'number';
  const ccActive = extras.branch !== undefined || extras.ccSessionId !== undefined;
  const repo = ccActive ? resolveRepoLabel(extras.repoId) : undefined;
  const appInfo = meta.codingAgentKind === 'app' ? resolveAppInfo(meta.codingAgentFolder) : undefined;

  return (
    <section class="route-section">
      <h4>Answered by</h4>
      {!hasContent && <div class="muted">Nothing recorded yet</div>}
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
            {contextLink(extras.contextTokens, extras.contextCapture)}
            {extras.contextTrimmed && <span class="pill"> older parts left out</span>}
          </span>
        </div>
      )}
      {repo && (
        <div class="route-row">
          <strong>Repository</strong>
          <span class={repo.failed ? 'error-text' : undefined}>{repo.text}</span>
        </div>
      )}
      {appInfo && extras.branch && (
        <div class="route-row">
          <strong>App</strong>
          <span class="route-value-group">
            <span class="route-app-icon" aria-hidden="true">{appInfo.icon}</span>
            <span class={appInfo.failed ? 'error-text' : undefined}>{appInfo.name}</span>
          </span>
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
          <strong>Session id</strong>
          <span class="mono">{extras.ccSessionId}</span>
        </div>
      )}
    </section>
  );
}

function contextLink(tokens: number, snapshot: ContextCapture | undefined): preact.JSX.Element | string {
  const label = `${tokens.toLocaleString()} tokens`;
  if (!snapshot) return label;
  return (
    <button
      type="button"
      class="accent-link"
      aria-label="Show the context sent for the last call of this turn"
      onClick={() => {
        closeMessageRoutePanel();
        contextViewer.value = { snapshot, description: 'Last call of this turn' };
      }}
    >
      {label}
    </button>
  );
}

export function resolveAppInfo(
  folder: string | undefined,
): { icon: ComponentChildren; name: string; failed?: boolean } | undefined {
  if (!folder) return undefined;
  const appId = folder.split('/').filter(Boolean).pop();
  if (!appId) return undefined;
  const apps = appsList.value;
  if (apps.status === 'not-loaded') void loadApps();
  const monogram = <AppIcon appId={appId} name={appId} />;
  if (apps.status === 'failed') {
    return { icon: monogram, name: `${appId} (apps failed to load)`, failed: true };
  }
  if (apps.status !== 'loaded') {
    return { icon: monogram, name: appId };
  }
  const match = apps.data.find(a => a.id === appId);
  const name = match?.name || appId;
  return { icon: <AppIcon appId={appId} name={name} icon={match?.icon} />, name };
}

function resolveRepoLabel(repoId: string | undefined): { text: string; failed?: boolean } | undefined {
  if (!repoId) return undefined;
  const repos = repositories.value;
  if (repos.status === 'not-loaded') void loadRepositories();
  if (repos.status === 'failed') return { text: 'Repository list failed to load', failed: true };
  if (repos.status !== 'loaded') return { text: '' };
  const match = repos.data.find(r => r.id === repoId);
  return match ? { text: match.name } : { text: 'A repository since removed' };
}

function renderTriggerOrigin(userEvent: Extract<StoredEvent, { type: 'TriggerStarted' }>) {
  const ts = triggers.value;
  const list = loadedOr(ts, []);
  const trigger = list.find(x => x.id === userEvent.trigger_id)
    ?? list.find(x => x.name === userEvent.trigger_name);
  const name = trigger?.name ?? userEvent.trigger_name ?? 'Unnamed trigger';
  const knownDeleted = ts.status === 'loaded' && !trigger;
  const invocation = userEvent.invocation;
  return (
    <>
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
          <strong>Fired when</strong>
          <span>{plainEventName(invocation.event_type)}</span>
        </div>
      )}
    </>
  );
}
