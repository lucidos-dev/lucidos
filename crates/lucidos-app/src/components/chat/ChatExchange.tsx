import type { ComponentChildren } from 'preact';
import { useEffect, useMemo, useRef } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { loadedOr } from '../../store/types';
import type { Loadable, ResponseEvent, App } from '../../store/types';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import type { Exchange } from '../../store/thread-events';
import {
  ENGINE_LABEL,
  LUCIDOS_AGENT_ICON,
  LUCIDOS_AGENT_LABEL,
  actorInitiator,
  exchangeUserMessage,
  exchangeUserImageHashes,
  exchangeTimestamp,
  exchangeResponseTimestamp,
  exchangeResponseText,
  exchangeSteps,
  exchangeResponseEvents,
  exchangeStatus,
  exchangeError,
  isEmptyContinuedExchange,
  findPermissionResolution,
  findQuestionAnswer,
  isChangeLifecycleEvent,
  modeToInitiator,
  originMode,
  responseAbortedSummary,
  resumeEngineNote,
} from '../../store/thread-events';
import type { Change } from '../../api/client';
import { continueThread, blobPreviewUrl } from '../../api/client';
import { getSessionBlobUrlForHash } from './pastedImages';
import { artifacts, appsList, openImagePopupFromThread, stepsExpanded, detailsExpanded, threadMap, findChangeById, lazyChanges, collapsedExchanges, toggleExchangeCollapsed, collapsedInitiators, toggleInitiatorCollapsed, toggleMessageRoutePanel, showToast, cancelingThreadIds } from '../../store/store';
import { ClaudeIcon } from '../shared/icons';
import { openFilePreview } from '../../store/actions/artifacts';
import { openApp } from '../../store/actions/apps';
import { revertChange, ensureChangeLoaded } from '../../store/actions/chat-changes';
import { viewChangeDiff } from '../../store/actions/repositories';
import { withScrollAnchor } from './CreateThreadView';
import { QuestionBody } from './QuestionCard';
import { PermissionBody } from './PermissionCard';
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
  /** True when this exchange is the most recent ResponseAborted with no
   *  later SessionRecovered in the thread — the only one that shows the
   *  Continue button. */
  isUnresumedAbort?: boolean;
}

export function ChatExchange({ exchange, streamingBuffer, isLast, threadId, hasPriorActive, imageOffset = 0, priorModel, priorEffort, isUnresumedAbort }: Props) {
  const rootRef = useRef<HTMLDivElement>(null);
  const showDetails = detailsExpanded.value;
  const showSteps = stepsExpanded.value;
  const artifactPaths = loadedOr(artifacts.value, NO_ARTIFACTS);
  const apps = loadedOr(appsList.value, NO_APPS);

  const userMessage = exchangeUserMessage(exchange);
  const userImageHashes = exchangeUserImageHashes(exchange);
  const timestamp = exchangeTimestamp(exchange);
  const responseTextRaw = exchangeResponseText(exchange);
  const threadMeta = threadMap.value.get(threadId)?.meta;
  const threadIsCC = threadMeta?.channel === 'claude_code';
  const threadIdle = threadMeta?.status === 'idle';
  const steps = exchangeSteps(exchange, isLast, threadIdle);
  const events = exchangeResponseEvents(exchange, imageOffset, isLast, threadIdle);
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
      if (src) openImagePopupFromThread(src, imgTarget);
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
        const app = apps.find((s: App) => s.id === appId);
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

  const exchangeActive = isStatusActive(status);
  const isEmptyContinued = isEmptyContinuedExchange(status, hasResponse, events, isLast);
  const isCanceling = exchangeActive && cancelingThreadIds.value.has(threadId);
  const sl = isCanceling
    ? { label: 'Canceling', className: 'working' }
    : getStatusLabel(status, hasSteps);
  const statusLabelText = sl.label;
  const statusClass = sl.className;
  const showStatus = exchangeActive || hasResponse || hasEvents || status === 'queued' || status === 'interrupted' || status === 'canceled' || status === 'error' || status === 'aborted';

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

  const initiator = useMemo(
    () => describeInitiator(exchange, userMessageHtml, userImageHashes, threadId),
    [exchange, userMessageHtml, userImageHashes, threadId],
  );
  const canCollapseInitiator = !!initiator.summary || !!initiator.details;
  const isInitiatorCollapsed = canCollapseInitiator
    && collapsedInitiators.value.has(`${threadId}:${exchange.userSeq}`);
  const isChangePanel = isChangeLifecycleEvent(exchange.userEvent);
  const isAbortPanel = exchange.userEvent.type === 'ResponseAborted';
  // Change lifecycle and abort-boundary exchanges are terminal — they have no
  // response, just the initiator panel with optional actions (Diff/Revert,
  // Continue).
  const showResponsePanel = !isChangePanel && !isAbortPanel && !isEmptyContinued && (hasResponse || hasEvents || showStatus);
  let initiatorActions: ComponentChildren | undefined;
  if (isChangePanel) {
    initiatorActions = changeActions(
      (exchange.userEvent as { change_id?: string }).change_id,
      exchange.userEvent.type === 'ChangeApplyFailed',
    );
  } else if (isAbortPanel && isUnresumedAbort) {
    initiatorActions = <ContinueButton threadId={threadId} />;
  }
  const executor = describeExecutor(threadIsCC);

  function renderResponseEvents(eventsList: ResponseEvent[]) {
    return eventsList.map((evt, i) => {
      if (evt.type === 'text' && evt.md?.trim()) {
        return <div key={`t${i}`} dangerouslySetInnerHTML={{ __html: visibleTextHtmls.get(evt)! }} />;
      }
      if (evt.type === 'step' && showSteps) return <InlineStep key={`s${i}`} event={evt} />;
      if (evt.type === 'image') return <GeneratedImage key={`img${i}`} event={evt} />;
      return null;
    });
  }

  return (
    <div class="chat-exchange" ref={rootRef}>
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
          hasBody={hasResponse || hasEvents}
          status={showStatus ? (
            <span class={`exchange-status-label exchange-status-${statusClass}`}>
              {statusLabelText}
              {statusClass === 'queued' && <span class="exchange-status-queued">{'○'}</span>}
              {statusClass === 'working' && <span class="mini-spinner" aria-hidden="true" />}
              {statusClass === 'waiting' && <span class="progress-dot progress-dot-waiting" />}
              {statusClass === 'awaiting' && <span class="exchange-status-awaiting">{'?'}</span>}
              {statusClass === 'done' && status !== 'interrupted' && <span class="exchange-status-check">{'✓'}</span>}
              {status === 'interrupted' && <span class="exchange-status-continued">{'↳'}</span>}
              {statusClass === 'canceled' && <span class="exchange-status-x">{'✕'}</span>}
              {statusClass === 'error' && <span class="exchange-status-x">{'✕'}</span>}
              {statusClass === 'aborted' && <span class="exchange-status-warning">{'⚠'}</span>}
            </span>
          ) : null}
          timestamp={formatMessageTimestamp(responseTimestamp || timestamp)}
          collapsible={canCollapse}
          collapsed={isCollapsed}
          onToggle={canCollapse
            ? () => toggleExchangeCollapsed(threadId, exchange.userSeq)
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

      {error && (
        <div class="exchange-error">
          <strong>Event stream error</strong>
          <p>{error}</p>
        </div>
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

export type InitiatorVariant = 'user' | 'system' | 'trigger' | 'lucidos';

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
    case 'SessionRecovered':         return 'Resumed after engine restart';
    case 'ResponseAborted':            return responseAbortedSummary(ev.actor);
    case 'MissingHardeningDetected': return 'Hardening required';
    case 'MergeConflictDetected':    return 'Merging changes from main';
    case 'CodingAgentPromptSent':    return 'Engine-injected prompt';
    case 'ChangeApplied':            return 'Change applied';
    case 'ChangeDiscarded':          return 'Change discarded';
    case 'ChangeReverted':           return 'Change reverted';
    case 'ChangeApplyFailed':        return 'Change failed';
    case 'UserPromptInjected':       return 'Auto-prompt sent';
    case 'MessageReceived':
      if (ev.origin?.kind === 'api') return 'API message';
      if (modeToInitiator(ev.mode) === 'system') return 'Forwarded message';
      return '';
    // Divider exchanges — the body component carries the question/permission
    // text, so the panel needs no separate summary line.
    case 'UserQuestionAsked':            return '';
    case 'CodingAgentPermissionRequest': return '';
    case 'CredentialRequested':          return `Credentials requested: ${ev.provider}`;
    case 'McpConsentRequested':          return `Tool consent requested: ${ev.tool}`;
    default:                         return '';
  }
}

/** Pick the panel variant for an event whose actor IS the initiator (forwarded
 *  message, child→parent callback). Engine-narrated events (change lifecycle,
 *  recovery) hardcode `'system'` regardless of the actor in their header. */
function actorVariant(actor: Parameters<typeof actorInitiator>[0]): InitiatorVariant {
  return originMode(actor) === 'agent' ? 'lucidos' : 'system';
}

/** Build a `'user'`-variant initiator descriptor with the standard human chip
 *  (icon + "You" label) and a caller-supplied summary/details/accent. Shared by
 *  every arm where the device-owner is the initiator (MessageReceived from a
 *  device, divider-starter ActionRequired events, …). */
function youInitiator(rest: Partial<InitiatorDescriptor> = {}): InitiatorDescriptor {
  return { variant: 'user', icon: '\u{1F464}', label: 'You', ...rest };
}

/** Build a `'system'`-variant descriptor with the engine chip (⚙ + Lucidos
 *  Engine). Shared by every arm where the engine narrates its own action
 *  (hardening / merge-conflict detection, legacy bare CC prompt). */
function engineInitiator(summary: string, details?: ComponentChildren): InitiatorDescriptor {
  return { variant: 'system', icon: '⚙', label: ENGINE_LABEL, summary, details };
}

export function describeInitiator(
  exchange: Exchange,
  userMessageHtml: string,
  userImageHashes: string[],
  threadId: string,
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
      // SessionRecovered carries an actor (device when triggered by Continue,
      // engine if auto-resume returns). Drive the chip from that actor.
      return {
        variant: actorVariant(ev.actor),
        ...actorInitiator(ev.actor),
        summary,
        details: <ResumeNoteBody exchange={exchange} />,
      };
    case 'ResponseAborted':
      // ResponseAborted is now an exchange boundary. Engine-attributed crashes
      // render '⚙ System'; device-attributed restarts render '👤 You'.
      return {
        variant: actorVariant(ev.actor),
        ...actorInitiator(ev.actor),
        summary,
      };
    case 'MissingHardeningDetected':
      return engineInitiator(summary);
    case 'MergeConflictDetected':
      return engineInitiator(
        summary,
        (ev.files?.length ?? 0) > 0 ? <FileList files={ev.files!} /> : undefined,
      );
    case 'CodingAgentPromptSent':
      // Reached only when the prompt has no preceding boundary (legacy
      // engine-spawned CC threads). Render the prompt text as the panel body
      // so the merge-conflict / hardening instructions are visible.
      return engineInitiator(
        summary,
        ev.text ? <MarkdownBlock html={renderMarkdown(ev.text)} /> : undefined,
      );
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
        variant: actorVariant(ev.origin),
        ...actorInitiator(ev.origin),
        summary,
        details: <MarkdownBlock html={userMessageHtml} />,
      };
    case 'MessageReceived': {
      const details = userMessageHtml || userImageHashes.length > 0
        ? <UserMessageBody html={userMessageHtml} imageHashes={userImageHashes} />
        : undefined;
      if (ev.origin?.kind === 'api' || modeToInitiator(ev.mode) === 'system') {
        return { variant: actorVariant(ev.origin), summary, details, ...actorInitiator(ev.origin) };
      }
      return youInitiator({ details });
    }
    case 'UserQuestionAsked': {
      // Resolution lives on this exchange's steps as UserQuestionAnswered;
      // matched by tool_use_id so a stale Answered from a different question
      // can't bleed in.
      const answered = findQuestionAnswer(exchange, ev.tool_use_id);
      return youInitiator({
        details: (
          <QuestionBody
            threadId={threadId}
            toolUseId={ev.tool_use_id}
            question={ev.question}
            options={ev.options ?? []}
            resolved={answered?.answer}
          />
        ),
      });
    }
    case 'CodingAgentPermissionRequest': {
      const resolvedStep = findPermissionResolution(exchange, ev.request_id);
      const resolved = resolvedStep
        ? { allowed: resolvedStep.allowed, reason: resolvedStep.reason }
        : undefined;
      return youInitiator({
        details: (
          <PermissionBody
            event={{
              request_id: ev.request_id,
              tool_use_id: ev.tool_use_id,
              tool_name: ev.tool_name,
              input: ev.input,
              summary: ev.summary,
            }}
            resolved={resolved}
          />
        ),
      });
    }
    case 'CredentialRequested':
    case 'McpConsentRequested':
      // Minimal divider rendering — chip + summary line. No body component
      // today; the engine surfaces these via separate transient flows.
      return youInitiator({ summary });
    default:
      // Unreachable in production (groupIntoExchanges only assigns starter
      // types to userEvent), but `userEvent: StoredEvent` covers every event
      // variant for legacy reasons, so TS can't enforce exhaustiveness here.
      return youInitiator();
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
      {files.map(f => <li key={f}><code>{f}</code></li>)}
    </ul>
  );
}

function UserMessageBody({ html, imageHashes }: { html: string; imageHashes: string[] }) {
  return (
    <>
      {html && <div class="markdown-content" dangerouslySetInnerHTML={{ __html: html }} />}
      {imageHashes.length > 0 && (
        <div class="user-images">
          {imageHashes.map((hash, i) => {
            const src = getSessionBlobUrlForHash(hash) ?? blobPreviewUrl(hash);
            return (
              <img
                key={hash + ':' + i}
                src={src}
                class="user-image-thumb"
                alt=""
                onClick={(e) => openImagePopupFromThread(e.currentTarget.src, e.currentTarget)}
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
  const change: Change | undefined = changeId ? findChangeById(changeId) : undefined;
  const lazy: Loadable<Change> = (changeId ? lazyChanges.value.get(changeId) : undefined) ?? { status: 'not-loaded' };
  const showLoading = useDelayedLoading(lazy);

  useEffect(() => {
    if (changeId) void ensureChangeLoaded(changeId);
  }, [changeId]);

  // Lifecycle error and live data both win over the lazy-fetch state — a 404
  // for a row that arrives via SSE moments later shouldn't strand a stale
  // "Failed to load" row beneath the now-resolved description.
  const lazyFailedError = !error && !change && lazy.status === 'failed'
    ? `Failed to load change details: ${lazy.error}`
    : undefined;
  const lazyLoading = !change && lazy.status === 'loading' && showLoading;

  const desc = change ? change.description.split('\n')[0] : undefined;
  const fileCount = change?.file_count;

  if (!desc && fileCount == null && !error && !lazyFailedError && !lazyLoading) return null;
  return (
    <div class="change-body">
      {desc && <div class="change-body-desc">{desc}</div>}
      {fileCount != null && (
        <div class="change-body-meta">{fileCount} file{fileCount !== 1 ? 's' : ''}</div>
      )}
      {error && <div class="change-body-error">{error}</div>}
      {lazyFailedError && <div class="change-body-error">{lazyFailedError}</div>}
      {lazyLoading && <div class="change-body-meta">Loading...</div>}
    </div>
  );
}

/** "Continue" button rendered on the most recent unresumed ResponseAborted
 *  exchange. Disables itself between click and response so a double-click
 *  can't double-emit. Surfaces network failures via toast and re-enables. */
function ContinueButton({ threadId }: { threadId: string }) {
  const inFlight = useSignal(false);
  const onClick = async (e: MouseEvent) => {
    e.stopPropagation();
    if (inFlight.value) return;
    inFlight.value = true;
    try {
      await continueThread(threadId);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      showToast(`Failed to continue: ${msg}`, 'error');
      inFlight.value = false;
      return;
    }
    // SessionRecovered will arrive via SSE and remove the button by hiding
    // this exchange's `isUnresumedAbort` — re-enable as a safety net in case
    // the SSE event is delayed.
    setTimeout(() => { inFlight.value = false; }, 5000);
  };
  return (
    <button class="action-btn" onClick={onClick} disabled={inFlight.value}>
      {inFlight.value ? 'Continuing...' : 'Continue'}
    </button>
  );
}

/** Render the engine note for a SessionRecovered exchange — a one-line
 *  subline followed by a `<details>` expansion showing the full injected text.
 *  Returns null when no engine note is present (e.g. CC resume path). */
function ResumeNoteBody({ exchange }: { exchange: Exchange }) {
  const note = resumeEngineNote(exchange);
  if (!note) return null;
  const subline = note.toolCount > 0
    ? `Reminded the model about ${note.toolCount} prior tool call${note.toolCount === 1 ? '' : 's'}`
    : 'Reminded the model that no actions had completed';
  return (
    <details class="resume-note">
      <summary>{subline}</summary>
      <pre class="resume-note-body">{note.text}</pre>
    </details>
  );
}

/** Diff/Revert action buttons rendered in the initiator panel's action slot
 *  for ChangeApplied/Discarded/Reverted exchanges. Returns null when the
 *  change has no relevant actions (e.g. ChangeApplyFailed leaves the change
 *  pending — user reads the error, doesn't diff/revert). */
function changeActions(changeId?: string, suppress?: boolean): ComponentChildren {
  if (suppress || !changeId) return null;
  const change = findChangeById(changeId);
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
  return { icon: LUCIDOS_AGENT_ICON, label: LUCIDOS_AGENT_LABEL };
}

interface ResponsePanelProps {
  executor: { icon: ComponentChildren; label: string };
  onExecutorClick?: (e: MouseEvent) => void;
  status: ComponentChildren;
  timestamp: string;
  collapsible: boolean;
  collapsed: boolean;
  onToggle?: () => void;
  hasBody: boolean;
  children: ComponentChildren;
}

function ResponsePanel({
  executor, onExecutorClick, status, timestamp, collapsible, collapsed, onToggle, hasBody, children,
}: ResponsePanelProps) {
  return (
    <div class={`response-panel${collapsed ? ' response-panel-collapsed' : ''}${hasBody ? '' : ' response-panel-bodyless'}`}>
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
          <span class="response-timestamp">{timestamp}</span>
        </span>
      </div>
      {hasBody && !collapsed && (
        <div class="response-body">
          {children}
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
  const tooltip = event.full !== event.description ? event.full : undefined;

  return (
    <div class={`inline-step ${statusClass}`} data-tooltip={tooltip}>
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
