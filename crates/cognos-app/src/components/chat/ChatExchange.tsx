import type { ComponentChildren } from 'preact';
import { useMemo, useRef, useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { loadedOr } from '../../store/types';
import type { ResponseEvent, App } from '../../store/types';
import type { Exchange } from '../../store/thread-events';
import {
  ENGINE_LABEL,
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
} from '../../store/thread-events';
import type { Change } from '../../api/client';
import { artifacts, appsList, popupImageSrc, stepsExpanded, detailsExpanded, threadMap, changes, appliedChanges, collapsedExchanges, toggleExchangeCollapsed, toggleMessageRoutePanel } from '../../store/store';
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
  const artifactPaths = loadedOr(artifacts.value, []);
  const apps = loadedOr(appsList.value, []);

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

  const initiator = describeInitiator(exchange, userMessageHtml, userImages);
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
      onClick={(e) => {
        e.stopPropagation();
        canceling.value = true;
        if (threadIsCC) {
          interruptCurrentExchange();
        } else {
          cancelCurrentExchange();
        }
      }}
    >
      <StopIcon />
    </button>
  ) : null;

  function renderResponseEvents(eventsList: ResponseEvent[]) {
    return eventsList.map((evt, i) => {
      if (evt.type === 'text' && evt.md?.trim()) {
        const html = renderMarkdown(evt.md);
        return <div key={`t${i}`} dangerouslySetInnerHTML={{ __html: linkifyPaths(html, artifactPaths, apps) }} />;
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
                  <div dangerouslySetInnerHTML={{
                    __html: linkifyPaths(collapsedFallbackText, artifactPaths, apps),
                  }} />
                ) : (
                  renderResponseEvents(visibleEvents)
                )
              ) : (
                <div dangerouslySetInnerHTML={{
                  __html: linkifyPaths(responseText, artifactPaths, apps),
                }} />
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
// User messages render the prompt; system-initiated exchanges (engine restart,
// auto-hardening, auto-merge, scheduled triggers) render a labeled placeholder.
// ---------------------------------------------------------------------------

export type InitiatorVariant = 'user' | 'system' | 'trigger';

export interface InitiatorDescriptor {
  variant: InitiatorVariant;
  icon: string;
  label: string;
  body?: ComponentChildren;
  /** Optional CSS modifier for status-specific accents (change-applied,
   *  change-failed, change-discarded, change-reverted). Stacks with `variant`. */
  accent?: string;
}

export function describeInitiator(
  exchange: Exchange,
  userMessageHtml: string,
  userImages: { base64: string; mimeType: string }[],
): InitiatorDescriptor {
  const ev = exchange.userEvent;
  switch (ev.type) {
    case 'TriggerStarted':
      return {
        variant: 'trigger',
        icon: '⏰',
        label: ev.trigger_name ? `Trigger: ${ev.trigger_name}` : 'Trigger',
        body: ev.prompt ? <MarkdownBlock html={renderMarkdown(ev.prompt)} /> : undefined,
      };
    case 'SessionRecovered':
      return { variant: 'system', icon: '↻', label: 'Engine restarted' };
    case 'MissingHardeningDetected':
      return { variant: 'system', icon: '⚙', label: ENGINE_LABEL, body: 'Hardening' };
    case 'MergeConflictDetected':
      return {
        variant: 'system',
        icon: '⚙',
        label: ENGINE_LABEL,
        body: (
          <>
            Merging changes from main
            {(ev.files?.length ?? 0) > 0 && <FileList files={ev.files!} />}
          </>
        ),
      };
    case 'ChangeApplied':
      return { variant: 'system', accent: 'change-applied', icon: '✓', label: 'Change applied', body: <ChangeBody changeId={ev.change_id} /> };
    case 'ChangeDiscarded':
      return { variant: 'system', accent: 'change-discarded', icon: '✕', label: 'Change discarded', body: <ChangeBody changeId={ev.change_id} /> };
    case 'ChangeReverted':
      return { variant: 'system', accent: 'change-reverted', icon: '↶', label: 'Change reverted', body: <ChangeBody changeId={ev.change_id} /> };
    case 'ChangeApplyFailed':
      return {
        variant: 'system',
        accent: 'change-failed',
        icon: '⚠',
        label: 'Change failed',
        body: <ChangeBody changeId={ev.change_id} error={ev.error} />,
      };
    case 'UserPromptInjected':
      return { variant: 'system', icon: '↪', label: 'Auto-prompt', body: <MarkdownBlock html={userMessageHtml} /> };
    case 'MessageReceived': {
      const body = userMessageHtml || userImages.length > 0
        ? <UserMessageBody html={userMessageHtml} images={userImages} />
        : undefined;
      const sender = ev.sender ?? ev.source;
      if (sender === 'system') {
        return { variant: 'system', icon: '⚙', label: 'System', body };
      }
      if (ev.origin?.kind === 'api') {
        return { variant: 'system', icon: '🔌', label: 'API', body };
      }
      return { variant: 'user', icon: '\u{1F464}', label: 'You', body };
    }
    default:
      // Unreachable in production (groupIntoExchanges only assigns starter
      // types to userEvent), but `userEvent: StoredEvent` covers every event
      // variant for legacy reasons, so TS can't enforce exhaustiveness here.
      return { variant: 'user', icon: '\u{1F464}', label: 'You' };
  }
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

interface InitiatorPanelProps {
  initiator: InitiatorDescriptor;
  timestamp: string;
  onActorClick?: (e: MouseEvent) => void;
  actions?: ComponentChildren;
}

function InitiatorPanel({ initiator, timestamp, onActorClick, actions }: InitiatorPanelProps) {
  const accentClass = initiator.accent ? ` initiator-panel-${initiator.accent}` : '';
  return (
    <div class={`initiator-panel initiator-panel-${initiator.variant}${accentClass}`}>
      <div class="initiator-header">
        <button
          type="button"
          class="initiator-actor"
          onClick={onActorClick}
          aria-label="Show initiator info"
        >
          <span class="initiator-icon">{initiator.icon}</span>
          <span class="initiator-label">{initiator.label}</span>
        </button>
        <span class="initiator-timestamp">{timestamp}</span>
      </div>
      {initiator.body && <div class="initiator-body">{initiator.body}</div>}
      {actions && <div class="initiator-footer">{actions}</div>}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Response panel — bordered card wrapping the executor's reply. Header carries
// executor + status + (optional) stop button + timestamp + collapse toggle.
// Body is the response content; collapses when the user clicks the header.
// ---------------------------------------------------------------------------

/** Triggers invoke Lucidos rather than running their own executor, so the label
 *  always reflects the engine that produced the response. The model is shown in
 *  the executor info popover, not in the header. */
export function describeExecutor(
  isCC: boolean,
): { icon: ComponentChildren; label: string } {
  if (isCC) return { icon: <ClaudeIcon />, label: 'Claude Code' };
  return { icon: '💡', label: 'Lucidos' };
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
  function handleHeaderClick(e: MouseEvent) {
    if (!onToggle) return;
    const target = e.target as HTMLElement;
    if (target.closest('button, a')) return;
    onToggle();
  }

  return (
    <div class={`response-panel${collapsed ? ' response-panel-collapsed' : ''}`}>
      <div
        class={`response-header${collapsible ? ' response-header-clickable' : ''}`}
        onClick={handleHeaderClick}
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
