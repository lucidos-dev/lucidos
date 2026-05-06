import type { ComponentChildren } from 'preact';
import { useMemo, useRef, useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { loadedOr } from '../../store/types';
import type { ResponseEvent, App } from '../../store/types';
import type { Exchange } from '../../store/thread-events';
import {
  ENGINE_LABEL,
  LUCIDOS_AGENT_LABEL,
  actorInitiator,
  exchangeUserMessage,
  exchangeUserImages,
  exchangeTimestamp,
  exchangeResponseTimestamp,
  exchangeResponseText,
  exchangeSteps,
  exchangeResponseEvents,
  exchangeStatus,
  exchangeError,
  isAbortedByRestart,
  isChangeLifecycleEvent,
  modeToInitiator,
} from '../../store/thread-events';
import type { Change } from '../../api/client';
import { artifacts, appsList, popupImageSrc, stepsExpanded, detailsExpanded, threadMap, changes, appliedChanges, collapsedExchanges, toggleExchangeCollapsed, collapsedInitiators, toggleInitiatorCollapsed, toggleMessageRoutePanel } from '../../store/store';
import { cancelCurrentExchange, interruptCurrentExchange } from '../../store/actions/chat';
import { StopIcon, ClaudeIcon } from '../shared/icons';
import { openFilePreview } from '../../store/actions/artifacts';
import { openApp } from '../../store/actions/apps';
import { revertChange } from '../../store/actions/chat-changes';
import { viewChangeDiff } from '../../store/actions/repositories';
import { withScrollAnchor } from './CreateThreadView';
import { QuestionCard } from './QuestionCard';
import { PermissionCard } from './PermissionCard';
import { getEventToggleState, getCollapsedVisibleEvents, splitEventSections } from '../../store/event-rendering';
import { statusLabel as getStatusLabel, isActive as isStatusActive } from '../../store/exchange-status';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { linkifyPaths } from '../../utils/linkifyPaths';

function formatTokens(n: number): string {
  if (n >= 1000) return `${Math.round(n / 1000)}k`;
  return String(n);
}

// Stable refs so loadedOr fallback doesn't yield a fresh [] each render —
// without these, every dependent useMemo invalidates on every render whenever
// artifacts/apps are not loaded.
const NO_ARTIFACTS: string[] = [];
const NO_APPS: App[] = [];

interface Props {
  exchange: Exchange;
  streamingBuffer: string;
  isLast: boolean;
  threadId: string;
  hasPriorActive?: boolean;
  imageOffset?: number;
  priorModel?: string;
  priorEffort?: string;
}

export function ChatExchange({ exchange, streamingBuffer, isLast, threadId, hasPriorActive, imageOffset = 0, priorModel, priorEffort }: Props) {
  const rootRef = useRef<HTMLDivElement>(null);
  const showDetails = detailsExpanded.value;
  const showSteps = stepsExpanded.value;
  const artifactPaths = loadedOr(artifacts.value, NO_ARTIFACTS);
  const apps = loadedOr(appsList.value, NO_APPS);

  const userMessage = exchangeUserMessage(exchange);
  const userImages = exchangeUserImages(exchange);
  const timestamp = exchangeTimestamp(exchange);
  const responseTextRaw = exchangeResponseText(exchange);
  const threadMeta = threadMap.value.get(threadId)?.meta;
  const threadIsCC = threadMeta?.channel === 'claude_code';
  const threadIdle = threadMeta?.status === 'idle';
  const steps = exchangeSteps(exchange, isLast, threadIdle);
  const events = exchangeResponseEvents(exchange, imageOffset, isLast);
  const status = exchangeStatus(exchange, streamingBuffer, isLast, hasPriorActive, threadIsCC, threadIdle);
  const error = exchangeError(exchange);

  const streamingHtml = streamingBuffer ? renderMarkdown(streamingBuffer) : '';
  const responseHtml = responseTextRaw ? renderMarkdown(responseTextRaw) : '';
  const responseText = streamingHtml || responseHtml;
  const hasResponse = !!responseText;

  const userMessageHtml = useMemo(
    () => linkifyPaths(renderMarkdown(userMessage), artifactPaths, apps),
    [userMessage, artifactPaths, apps],
  );

  const hasEvents = events.length > 0;
  const hasSections = events.some(e => e.type === 'section_break');
  const { showMoreToggle, showStepsToggle } = getEventToggleState(events);
  const hasSteps = steps.length > 0 || events.some(e => e.type === 'step');

  const canCollapse = hasResponse || hasEvents;
  const isCollapsed = canCollapse && collapsedExchanges.value.has(`${threadId}:${exchange.userSeq}`);

  function handleLinkClick(e: MouseEvent) {
    const imgTarget = (e.target as HTMLElement).closest('.image-thumbnail') as HTMLImageElement | null;
    if (imgTarget) {
      e.preventDefault();
      const src = imgTarget.dataset.fullSrc || imgTarget.src;
      if (src) popupImageSrc.value = src;
      return;
    }

    const artifactTarget = (e.target as HTMLElement).closest('.artifact-link') as HTMLElement | null;
    if (artifactTarget) {
      e.preventDefault();
      const path = artifactTarget.dataset.path;
      if (path) openFilePreview(path);
      return;
    }

    const appTarget = (e.target as HTMLElement).closest('.app-link') as HTMLElement | null;
    if (appTarget) {
      e.preventDefault();
      const appId = appTarget.dataset.appId;
      if (appId) {
        const loadedApps = loadedOr(appsList.value, []);
        const app = loadedApps.find((s: App) => s.id === appId);
        if (app) openApp(app);
      }
      return;
    }
  }

  function toggleDetails() {
    withScrollAnchor(rootRef.current, () => {
      detailsExpanded.value = !detailsExpanded.value;
    });
  }

  function toggleSteps() {
    withScrollAnchor(rootRef.current, () => {
      stepsExpanded.value = !stepsExpanded.value;
    });
  }

  const canceling = useSignal(false);
  useEffect(() => {
    if (!isStatusActive(status)) canceling.value = false;
  }, [status]);

  const exchangeActive = isStatusActive(status);
  const showAsContinued = status === 'done' && !hasResponse && !hasEvents && !isLast;
  const displayStatus = showAsContinued ? 'interrupted' : status;
  const sl = canceling.value
    ? { label: 'Canceling', className: 'working' }
    : getStatusLabel(displayStatus, hasSteps);
  const statusLabelText = sl.label;
  const statusClass = sl.className;
  const showStatus = exchangeActive || hasResponse || hasEvents || showAsContinued || status === 'queued' || status === 'interrupted' || status === 'canceled' || status === 'error' || status === 'aborted';

  const responseTimestamp = exchangeResponseTimestamp(exchange);

  function openInfoPanel(section: 'origin' | 'executor', e: MouseEvent) {
    e.stopPropagation();
    toggleMessageRoutePanel({
      anchor: e.currentTarget as HTMLElement,
      exchange,
      threadId,
      section,
      priorModel,
      priorEffort,
    });
  }

  function renderToggles() {
    if (!showMoreToggle && !showStepsToggle) return null;
    return (
      <div class="response-toggles">
        {showMoreToggle && (
          <button class="details-toggle" onClick={toggleDetails}>
            {showDetails ? 'Less' : 'More'}
          </button>
        )}
        {showStepsToggle && (
          <button class="details-toggle" onClick={toggleSteps}>
            {showSteps ? 'Hide steps' : 'Show steps'}
          </button>
        )}
      </div>
    );
  }

  const { visibleEvents, collapsedFallbackText } = useMemo(() => {
    let visible: ResponseEvent[] = [];
    let fallback = '';
    if (hasEvents) {
      if (showDetails || !showMoreToggle) {
        visible = events;
      } else {
        const collapsed = getCollapsedVisibleEvents(events);
        visible = collapsed.visibleEvents;
        if (collapsed.needsFallback) {
          fallback = responseText;
        }
      }
    }
    return { visibleEvents: visible, collapsedFallbackText: fallback };
  }, [hasEvents, showDetails, showMoreToggle, events, responseText]);

  // Memoize linkified HTML — linkifyPaths builds 15+ regex batches per call when
  // the workspace has many artifacts. Without memoization, every re-render of
  // this exchange (signal fire from threadMap/artifacts/appsList during SSE
  // activity) reruns the full scan and blocks the main thread.
  const visibleTextHtmls = useMemo(() => {
    const map = new Map<ResponseEvent, string>();
    for (const evt of visibleEvents) {
      if (evt.type === 'text' && evt.md?.trim()) {
        map.set(evt, linkifyPaths(renderMarkdown(evt.md), artifactPaths, apps));
      }
    }
    return map;
  }, [visibleEvents, artifactPaths, apps]);

  const collapsedFallbackHtml = useMemo(
    () => linkifyPaths(collapsedFallbackText, artifactPaths, apps),
    [collapsedFallbackText, artifactPaths, apps],
  );

  const responseTextHtml = useMemo(
    () => linkifyPaths(responseText, artifactPaths, apps),
    [responseText, artifactPaths, apps],
  );

  const initiator = describeInitiator(exchange, userMessageHtml, userImages);
  const canCollapseInitiator = !!initiator.summary || !!initiator.details;
  const isInitiatorCollapsed = canCollapseInitiator
    && collapsedInitiators.value.has(`${threadId}:${exchange.userSeq}`);
  const isChangePanel = isChangeLifecycleEvent(exchange.userEvent);
  // Change lifecycle exchanges are terminal — they have no response, just the
  // initiator panel with optional Diff/Revert actions and a body for description/error.
  const showResponsePanel = !isChangePanel && (hasResponse || hasEvents || showStatus);
  const initiatorActions = isChangePanel
    ? changeActions(
        (exchange.userEvent as { change_id?: string }).change_id,
        exchange.userEvent.type === 'ChangeApplyFailed',
      )
    : undefined;
  const executor = describeExecutor(threadIsCC);

  const stopBtn = exchangeActive && !canceling.value ? (
    <button
      class="exchange-stop-btn"
      data-tooltip="Stop"
      onClick={async (e) => {
        e.stopPropagation();
        canceling.value = true;
        const ok = threadIsCC
          ? await interruptCurrentExchange()
          : await cancelCurrentExchange();
        if (!ok) canceling.value = false;
      }}
    >
      <StopIcon />
    </button>
  ) : null;

  function renderResponseEvents(eventsList: ResponseEvent[]) {
    return eventsList.map((evt, i) => {
      if (evt.type === 'text' && evt.md?.trim()) {
        return <div key={`t${i}`} dangerouslySetInnerHTML={{ __html: visibleTextHtmls.get(evt)! }} />;
      }
      if (evt.type === 'step' && showSteps) return <InlineStep key={`s${i}`} event={evt} />;
      if (evt.type === 'image') return <GeneratedImage key={`img${i}`} event={evt} />;
      if (evt.type === 'question') return <QuestionCard key={`q${i}`} threadId={threadId} event={evt} />;
      if (evt.type === 'permission') return <PermissionCard key={`p${i}`} event={evt} />;
      return null;
    });
  }

  return (
    <div class="chat-exchange" ref={rootRef}>
      {error && (
        <div class="exchange-error">
          <strong>Event stream error</strong>
          <p>{error}</p>
        </div>
      )}

      <InitiatorPanel
        initiator={initiator}
        timestamp={formatMessageTimestamp(timestamp)}
        onActorClick={(e) => openInfoPanel('origin', e)}
        actions={initiatorActions}
        collapsible={canCollapseInitiator}
        collapsed={isInitiatorCollapsed}
        onToggle={canCollapseInitiator
          ? () => toggleInitiatorCollapsed(threadId, exchange.userSeq)
          : undefined}
      />

      {showResponsePanel && (
        <ResponsePanel
          executor={executor}
          onExecutorClick={(e) => openInfoPanel('executor', e)}
          status={showStatus ? (
            <span class={`exchange-status-label exchange-status-${statusClass}`}>
              {statusLabelText}
              {statusClass === 'queued' && <span class="exchange-status-queued">{'○'}</span>}
              {statusClass === 'waiting' && <span class="progress-dot progress-dot-waiting" />}
              {statusClass === 'awaiting' && <span class="exchange-status-awaiting">{'?'}</span>}
              {statusClass === 'done' && displayStatus !== 'interrupted' && <span class="exchange-status-check">{'✓'}</span>}
              {displayStatus === 'interrupted' && <span class="exchange-status-continued">{'↳'}</span>}
              {statusClass === 'canceled' && <span class="exchange-status-x">{'✕'}</span>}
              {statusClass === 'error' && <span class="exchange-status-x">{'✕'}</span>}
              {statusClass === 'aborted' && <span class="exchange-status-warning">{'⚠'}</span>}
            </span>
          ) : null}
          timestamp={formatMessageTimestamp(responseTimestamp || timestamp)}
          stopBtn={stopBtn}
          collapsible={canCollapse}
          collapsed={isCollapsed}
          onToggle={canCollapse
            ? () => toggleExchangeCollapsed(threadId, exchange.userSeq)
            : undefined}
          aborted={status === 'aborted' && (hasResponse || hasEvents)
            ? (isAbortedByRestart(exchange) ? 'Engine restart interrupted this response' : 'Response interrupted')
            : undefined}
        >
          {hasEvents && hasSections ? (
            splitEventSections(visibleEvents).map((section, sIdx) => (
              <div class="response-content markdown-content" key={`sec${sIdx}`} onClick={handleLinkClick}>
                {sIdx === 0 && renderToggles()}
                {renderResponseEvents(section)}
              </div>
            ))
          ) : (
            <div class="response-content markdown-content" onClick={handleLinkClick}>
              {renderToggles()}
              {hasEvents ? (
                collapsedFallbackText ? (
                  <div dangerouslySetInnerHTML={{ __html: collapsedFallbackHtml }} />
                ) : (
                  renderResponseEvents(visibleEvents)
                )
              ) : (
                <div dangerouslySetInnerHTML={{ __html: responseTextHtml }} />
              )}
            </div>
          )}
        </ResponsePanel>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Initiator panel — bordered card describing who/what started this exchange.
//
// Every panel reads as "[icon] WHO — WHAT": the label is the initiator's name
// (Lucidos Engine, You, trigger name) and the summary is a one-line action
// description (Hardening required, Change applied, Auto-prompt sent). Rich
// payloads (message text, change description, file list) go in `details`.
// Click the actor to open the route popover for finer origin info.
// ---------------------------------------------------------------------------

export type InitiatorVariant = 'user' | 'system' | 'trigger';

export interface InitiatorDescriptor {
  variant: InitiatorVariant;
  icon: string;
  /** WHO performed this — always the initiator's display name. */
  label: string;
  /** WHAT was done — short action description shown as the panel's lead line.
   *  Omitted for user messages where the message itself is the content. */
  summary?: string;
  /** Optional richer payload (message text/images, change description, file list)
   *  rendered below the summary. */
  details?: ComponentChildren;
  /** Optional CSS modifier for status-specific accents (change-applied,
   *  change-failed, change-discarded, change-reverted). Stacks with `variant`. */
  accent?: string;
}

/** Action label shared by the panel header and the route popover's Origin row. */
export function initiatorSummary(ev: Exchange['userEvent']): string {
  switch (ev.type) {
    case 'TriggerStarted':           return 'Trigger fired';
    case 'SessionRecovered':         return 'Engine restarted';
    case 'MissingHardeningDetected': return 'Hardening required';
    case 'MergeConflictDetected':    return 'Merging changes from main';
    case 'ChangeApplied':            return 'Change applied';
    case 'ChangeDiscarded':          return 'Change discarded';
    case 'ChangeReverted':           return 'Change reverted';
    case 'ChangeApplyFailed':        return 'Change failed';
    case 'UserPromptInjected':       return 'Auto-prompt sent';
    case 'MessageReceived':
      if (ev.origin?.kind === 'api') return 'API message';
      if (modeToInitiator(ev.mode) === 'system') return 'Forwarded message';
      return '';
    default:                         return '';
  }
}

export function describeInitiator(
  exchange: Exchange,
  userMessageHtml: string,
  userImages: { base64: string; mimeType: string }[],
): InitiatorDescriptor {
  const ev = exchange.userEvent;
  const summary = initiatorSummary(ev);
  switch (ev.type) {
    case 'TriggerStarted':
      return {
        variant: 'trigger',
        icon: '⏰',
        label: ENGINE_LABEL,
        summary,
        details: ev.prompt ? <MarkdownBlock html={renderMarkdown(ev.prompt)} /> : undefined,
      };
    case 'SessionRecovered':
    case 'MissingHardeningDetected':
      return { variant: 'system', icon: '⚙', label: ENGINE_LABEL, summary };
    case 'MergeConflictDetected':
      return {
        variant: 'system',
        icon: '⚙',
        label: ENGINE_LABEL,
        summary,
        details: (ev.files?.length ?? 0) > 0 ? <FileList files={ev.files!} /> : undefined,
      };
    case 'ChangeApplied':
    case 'ChangeDiscarded':
    case 'ChangeReverted':
      return {
        variant: 'system', accent: changeAccent(ev.type),
        ...actorInitiator(ev.actor),
        summary,
        details: <ChangeBody changeId={ev.change_id} />,
      };
    case 'ChangeApplyFailed':
      return {
        variant: 'system', accent: 'change-failed',
        ...actorInitiator(ev.actor),
        summary,
        details: <ChangeBody changeId={ev.change_id} error={ev.error} />,
      };
    case 'UserPromptInjected':
      // Legacy rows lack `origin` and fall back to the engine label.
      return {
        variant: 'system',
        ...actorInitiator(ev.origin),
        summary,
        details: <MarkdownBlock html={userMessageHtml} />,
      };
    case 'MessageReceived': {
      const details = userMessageHtml || userImages.length > 0
        ? <UserMessageBody html={userMessageHtml} images={userImages} />
        : undefined;
      if (ev.origin?.kind === 'api' || modeToInitiator(ev.mode) === 'system') {
        return { variant: 'system', summary, details, ...actorInitiator(ev.origin) };
      }
      return { variant: 'user', icon: '\u{1F464}', label: 'You', details };
    }
    default:
      // Unreachable in production (groupIntoExchanges only assigns starter
      // types to userEvent), but `userEvent: StoredEvent` covers every event
      // variant for legacy reasons, so TS can't enforce exhaustiveness here.
      return { variant: 'user', icon: '\u{1F464}', label: 'You' };
  }
}

const CHANGE_ACCENT = {
  ChangeApplied: 'change-applied',
  ChangeDiscarded: 'change-discarded',
  ChangeReverted: 'change-reverted',
} as const;

function changeAccent(type: keyof typeof CHANGE_ACCENT): string {
  return CHANGE_ACCENT[type];
}

function MarkdownBlock({ html }: { html: string }) {
  return <div class="markdown-content" dangerouslySetInnerHTML={{ __html: html }} />;
}

function FileList({ files }: { files: string[] }) {
  return (
    <ul class="initiator-files">
      {files.map(f => <li><code>{f}</code></li>)}
    </ul>
  );
}

function UserMessageBody({ html, images }: { html: string; images: { base64: string; mimeType: string }[] }) {
  return (
    <>
      {html && <div class="markdown-content" dangerouslySetInnerHTML={{ __html: html }} />}
      {images.length > 0 && (
        <div class="user-images">
          {images.map((img, i) => {
            const src = `data:${img.mimeType};base64,${img.base64}`;
            return (
              <img
                key={i}
                src={src}
                class="user-image-thumb"
                alt=""
                onClick={() => { popupImageSrc.value = src; }}
              />
            );
          })}
        </div>
      )}
    </>
  );
}

/** Body for change lifecycle initiator panels — surfaces the change description,
 *  file count, and (when failed) the error message. The resolved timestamp is
 *  the panel header timestamp (the change-lifecycle event time IS resolved_at). */
function ChangeBody({ changeId, error }: { changeId?: string; error?: string }) {
  const change: Change | undefined = changeId
    ? (changes.value.find(c => c.id === changeId)
       || appliedChanges.value.find(c => c.id === changeId))
    : undefined;

  const desc = change ? change.description.split('\n')[0] : undefined;
  const fileCount = change?.file_count;

  if (!desc && fileCount == null && !error) return null;
  return (
    <div class="change-body">
      {desc && <div class="change-body-desc">{desc}</div>}
      {fileCount != null && (
        <div class="change-body-meta">{fileCount} file{fileCount !== 1 ? 's' : ''}</div>
      )}
      {error && <div class="change-body-error">{error}</div>}
    </div>
  );
}

/** Diff/Revert action buttons rendered in the initiator panel's action slot
 *  for ChangeApplied/Discarded/Reverted exchanges. Returns null when the
 *  change has no relevant actions (e.g. ChangeApplyFailed leaves the change
 *  pending — user reads the error, doesn't diff/revert). */
function changeActions(changeId?: string, suppress?: boolean): ComponentChildren {
  if (suppress || !changeId) return null;
  const change = changes.value.find(c => c.id === changeId)
    || appliedChanges.value.find(c => c.id === changeId);
  if (!change) return null;
  const showDiff = change.status === 'pending' || !!change.pre_merge_sha;
  const showRevert = change.status === 'applied';
  if (!showDiff && !showRevert) return null;
  return (
    <>
      {showDiff && <button class="action-btn" onClick={() => viewChangeDiff(change)}>Diff</button>}
      {showRevert && <button class="action-btn action-btn-danger" onClick={() => revertChange(change.id)}>Revert</button>}
    </>
  );
}

/** Shared header click handler for InitiatorPanel/ResponsePanel — toggles the
 *  panel's collapsed state, but ignores clicks that originated on a button or
 *  link inside the header (actor info, executor info, stop). */
function handlePanelHeaderClick(e: MouseEvent, onToggle?: () => void): void {
  if (!onToggle) return;
  if ((e.target as HTMLElement).closest('button, a')) return;
  onToggle();
}

interface InitiatorPanelProps {
  initiator: InitiatorDescriptor;
  timestamp: string;
  onActorClick?: (e: MouseEvent) => void;
  actions?: ComponentChildren;
  collapsible: boolean;
  collapsed: boolean;
  onToggle?: () => void;
}

function InitiatorPanel({ initiator, timestamp, onActorClick, actions, collapsible, collapsed, onToggle }: InitiatorPanelProps) {
  const accentClass = initiator.accent ? ` initiator-panel-${initiator.accent}` : '';
  const hasBody = !!initiator.summary || !!initiator.details;

  return (
    <div class={`initiator-panel initiator-panel-${initiator.variant}${accentClass}${collapsed ? ' initiator-panel-collapsed' : ''}`}>
      <div
        class={`initiator-header${collapsible ? ' initiator-header-clickable' : ''}`}
        onClick={(e) => handlePanelHeaderClick(e, onToggle)}
      >
        <button
          type="button"
          class="initiator-actor"
          onClick={onActorClick}
          aria-label={`Show info for ${initiator.label}`}
        >
          <span class="initiator-icon">{initiator.icon}</span>
          <span class="initiator-label">{initiator.label}</span>
        </button>
        <span class="initiator-timestamp">{timestamp}</span>
      </div>
      {hasBody && !collapsed && (
        <div class="initiator-body">
          {initiator.summary && <div class="initiator-summary">{initiator.summary}</div>}
          {initiator.details}
        </div>
      )}
      {actions && !collapsed && <div class="initiator-footer">{actions}</div>}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Response panel — bordered card wrapping the executor's reply. Header carries
// executor + status + (optional) stop button + timestamp + collapse toggle.
// Body is the response content; collapses when the user clicks the header.
// ---------------------------------------------------------------------------

/** Triggers invoke the Lucidos agent rather than running their own executor,
 *  so the label always reflects the LLM that produced the response. The model
 *  is shown in the executor info popover, not in the header. The "Lucidos
 *  Agent" name matches the initiator label used when one Lucidos thread
 *  spawns another — same entity, same display. */
export function describeExecutor(
  isCC: boolean,
): { icon: ComponentChildren; label: string } {
  if (isCC) return { icon: <ClaudeIcon />, label: 'Claude Code' };
  return { icon: '💡', label: LUCIDOS_AGENT_LABEL };
}

interface ResponsePanelProps {
  executor: { icon: ComponentChildren; label: string };
  onExecutorClick?: (e: MouseEvent) => void;
  status: ComponentChildren;
  timestamp: string;
  stopBtn: ComponentChildren;
  collapsible: boolean;
  collapsed: boolean;
  onToggle?: () => void;
  aborted?: string;
  children: ComponentChildren;
}

function ResponsePanel({
  executor, onExecutorClick, status, timestamp, stopBtn, collapsible, collapsed, onToggle, aborted, children,
}: ResponsePanelProps) {
  return (
    <div class={`response-panel${collapsed ? ' response-panel-collapsed' : ''}`}>
      <div
        class={`response-header${collapsible ? ' response-header-clickable' : ''}`}
        onClick={(e) => handlePanelHeaderClick(e, onToggle)}
      >
        <button
          type="button"
          class="response-executor"
          onClick={onExecutorClick}
          aria-label="Show executor info"
        >
          <span class="response-executor-icon">{executor.icon}</span>
          <span class="response-executor-label">{executor.label}</span>
        </button>
        <span class="response-meta">
          {status}
          {stopBtn}
          <span class="response-timestamp">{timestamp}</span>
        </span>
      </div>
      {!collapsed && (
        <div class="response-body">
          {children}
          {aborted && (
            <div class="response-aborted-marker">
              <span class="response-aborted-icon">{'⚠'}</span>
              <span>{aborted}</span>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/** Generated image rendered inline in the response. */
function GeneratedImage({ event }: { event: Extract<ResponseEvent, { type: 'image' }> }) {
  const src = `data:${event.mime_type};base64,${event.base64}`;
  return (
    <div class="generated-image" data-tooltip={`thread:${event.index}`}>
      <img
        src={src}
        class="image-thumbnail"
        alt="Generated image"
        loading="lazy"
      />
    </div>
  );
}

/** Compact inline step rendered between text blocks. */
function InlineStep({ event }: { event: Extract<ResponseEvent, { type: 'step' }> }) {
  const statusClass = event.success === null ? 'pending' : event.success ? 'success' : 'error';
  const hasContext = event.context_tokens != null;

  return (
    <div class={`inline-step ${statusClass}`}>
      <span class="step-icon">
        {event.success === null ? <span class="mini-spinner" /> : event.success ? '✓' : '⚠'}
      </span>
      <span class="step-description">{event.description}</span>
      {event.detail && <span class="step-detail">{event.detail}</span>}
      {hasContext && (
        <span class={`step-context${event.trimmed ? ' trimmed' : ''}`}>
          {formatTokens(event.context_tokens!)} tokens, {event.context_messages} msgs
          {event.trimmed && ' (trimmed)'}
        </span>
      )}
    </div>
  );
}
