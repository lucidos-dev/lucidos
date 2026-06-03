import type { ComponentChildren } from 'preact';
import { memo } from 'preact/compat';
import { useMemo, useRef } from 'preact/hooks';
import { loadedOr } from '../../store/types';
import type { ResponseEvent, App } from '../../store/types';
import type { Exchange, ThreadEvent } from '../../store/thread-events';
import { ENGINE_LABEL, LUCIDOS_AGENT_ICON, LUCIDOS_AGENT_LABEL, actorInitiator, exchangeUserMessage, exchangeUserImageHashes, exchangeTimestamp, exchangeResponseTimestamp, exchangeResponseText, exchangeEngineLimitDetail, exchangeSteps, exchangeResponseEvents, exchangeStatus, exchangeError, isEmptyContinuedExchange, isCanceledQuestionDivider, findPermissionResolution, findQuestionAnswer, isChangeLifecycleEvent, modeToInitiator, originMode, responseAbortedSummary, RESPONSE_CANCELED_SUMMARY } from '../../store/thread-events';
import { artifacts, appsList, openImagePopupFromGroup, stepsExpanded, detailsExpanded, collapsedExchanges, toggleExchangeCollapsed, collapsedInitiators, toggleInitiatorCollapsed, toggleMessageRoutePanel } from '../../store/store';
import { preserveOnToggle } from './scrollState';
import { openFilePreview } from '../../store/actions/artifacts';
import { openApp } from '../../store/actions/apps';
import { withScrollAnchor } from './CreateThreadView';
import { QuestionBody } from './QuestionCard';
import { PermissionBody } from './PermissionCard';
import { ChildCompletionCard } from './ChildCompletionCard';
import { getEventToggleState, getCollapsedVisibleEvents, splitEventSections } from '../../store/event-rendering';
import { statusLabel as getStatusLabel, isActive as isStatusActive, isTerminated } from '../../store/exchange-status';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { linkifyPaths, extractAppIdFromHref, extractNavTargetFromHref } from '../../utils/linkifyPaths';
import { handleNavigationRequest } from '../../store/actions/thread-sync';
import { ChangeBody, ContinueButton, FileList, GeneratedImage, InitiatorPanel, InlineStep, MarkdownBlock, ResponsePanel, ResumeNoteBody, UserMessageBody, changeAccent, changeActions, describeExecutor } from './chat-exchange-parts';

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
   *  later ContinuationStarted in the thread — the only one that shows the
   *  Continue button. */
  isUnresumedAbort?: boolean;
  /** Lifted from `threadMap.value.get(threadId)?.meta.channel === 'claude_code'`
   *  in `renderExchanges` so this component does not subscribe to threadMap
   *  itself — see `chatExchangePropsEqual` below for the memo contract. */
  threadIsCC: boolean;
  /** Lifted from `isThreadQuiescent(threadMap.value.get(threadId)?.meta.status)`. */
  threadIdle: boolean;
  /** Lifted from `cancelingThreadIds.value.has(threadId)`. */
  threadCanceling: boolean;
}

function ChatExchangeImpl({ exchange, streamingBuffer, isLast, threadId, hasPriorActive, imageOffset = 0, priorModel, priorEffort, isUnresumedAbort, threadIsCC, threadIdle, threadCanceling }: Props) {
  const rootRef = useRef<HTMLDivElement>(null);
  const showDetails = detailsExpanded.value;
  const showSteps = stepsExpanded.value;
  const artifactPaths = loadedOr(artifacts.value, NO_ARTIFACTS);
  const apps = loadedOr(appsList.value, NO_APPS);

  const userMessage = exchangeUserMessage(exchange);
  const userImageHashes = exchangeUserImageHashes(exchange);
  const timestamp = exchangeTimestamp(exchange);
  const responseTextRaw = exchangeResponseText(exchange);
  const steps = exchangeSteps(exchange, isLast, threadIdle);
  const events = exchangeResponseEvents(exchange, imageOffset, isLast, threadIdle);
  const status = exchangeStatus(exchange, streamingBuffer, isLast, hasPriorActive, threadIsCC, threadIdle);
  const error = exchangeError(exchange);

  // Cap detection reads ResponseGenerated.text directly via exchangeEngineLimitDetail —
  // the cap is emitted without a preceding TextStreamed, so it never lands in
  // responseTextRaw (which only concatenates streamed text). Without this side
  // channel the agent appears to stop silently mid-task.
  const engineLimitDetail = !streamingBuffer ? exchangeEngineLimitDetail(exchange) : '';
  const isEngineLimit = !!engineLimitDetail;
  const streamingHtml = streamingBuffer ? renderMarkdown(streamingBuffer) : '';
  const responseHtml = responseTextRaw ? renderMarkdown(responseTextRaw) : '';
  const responseHtmlCombined = streamingHtml || responseHtml;
  const hasResponse = !!responseHtmlCombined || isEngineLimit;

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
      if (src) openImagePopupFromGroup(src, imgTarget);
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
        const app = apps.find((a) => a.id === appId);
        if (app) openApp(app);
      }
      return;
    }

    const navTarget = (e.target as HTMLElement).closest('.nav-link') as HTMLElement | null;
    if (navTarget) {
      e.preventDefault();
      const target = navTarget.dataset.navTarget;
      if (target) handleNavigationRequest({ target });
      return;
    }

    // Defense-in-depth: intercept plain anchors whose href points at an app
    // folder (apps/<id>/...) or a Lucidos navigation panel
    // (notifications, apps, triggers, …) even when linkifyPaths didn't
    // rewrite them. Catches: stale memo result rendered before the apps
    // list loaded; iOS PWA JS bundle predating the rewriter; any markdown
    // link the LLM writes. Without this, the browser would navigate to
    // the relative URL — for app entries to a file preview (via the
    // engine's /data/* static mount), for panel names to a 404 on a
    // non-existent /data/<panel> folder.
    const anchorTarget = (e.target as HTMLElement).closest('a') as HTMLAnchorElement | null;
    if (anchorTarget) {
      const rawHref = anchorTarget.getAttribute('href') || '';
      const appId = extractAppIdFromHref(rawHref);
      if (appId) {
        const app = apps.find((a) => a.id === appId);
        if (app) {
          e.preventDefault();
          openApp(app);
          return;
        }
      }
      const navName = extractNavTargetFromHref(rawHref);
      if (navName) {
        e.preventDefault();
        handleNavigationRequest({ target: navName });
        return;
      }
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
  const isCanceling = exchangeActive && threadCanceling;
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
          fallback = responseHtmlCombined;
        }
      }
    }
    return { visibleEvents: visible, collapsedFallbackText: fallback };
  }, [hasEvents, showDetails, showMoreToggle, events, responseHtmlCombined]);

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

  const responseHtmlLinkified = useMemo(
    () => linkifyPaths(responseHtmlCombined, artifactPaths, apps),
    [responseHtmlCombined, artifactPaths, apps],
  );

  const responseTerminated = isTerminated(status) || exchange.questionOvertaken === true;

  const initiator = useMemo(
    () => describeInitiator(exchange, userMessageHtml, userImageHashes, threadId, responseTerminated),
    [exchange, userMessageHtml, userImageHashes, threadId, responseTerminated],
  );
  const canCollapseInitiator = !!initiator.summary || !!initiator.details;
  const isInitiatorCollapsed = canCollapseInitiator
    && collapsedInitiators.value.has(`${threadId}:${exchange.userSeq}`);
  const isChangePanel = isChangeLifecycleEvent(exchange.userEvent);
  const isAbortPanel = exchange.userEvent.type === 'ResponseAborted';
  const isCancelPanel = exchange.userEvent.type === 'ResponseCanceled';
  const isCanceledDivider = isCanceledQuestionDivider(exchange);
  // Change lifecycle, abort-boundary, cancel-boundary, and canceled-question-
  // divider exchanges are terminal — they have no response, just the initiator
  // panel with optional actions (Diff/Revert on change panels, Continue on the
  // unresumed abort, the QuestionCard's own ✓ Cancel button on the divider).
  const showResponsePanel = !isChangePanel && !isAbortPanel && !isCancelPanel && !isCanceledDivider && !isEmptyContinued && (hasResponse || hasEvents || showStatus);
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
    <div class="chat-exchange" ref={rootRef} data-event-id={exchange.userEvent._eventId}>
      <InitiatorPanel
        initiator={initiator}
        timestamp={formatMessageTimestamp(timestamp)}
        onActorClick={initiator.actorClickable === false
          ? undefined
          : (e) => openInfoPanel('origin', e)}
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
          status={showStatus && shouldShowResponseStatusBadge(exchange.userEvent.type, statusClass) ? (
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
            ? () => {
                preserveOnToggle();
                toggleExchangeCollapsed(threadId, exchange.userSeq);
              }
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
                <div dangerouslySetInnerHTML={{ __html: responseHtmlLinkified }} />
              )}
            </div>
          )}
          {isEngineLimit && (
            <div class="exchange-engine-limit" role="status">
              <strong>Per-turn cap reached</strong>
              <p>{engineLimitDetail}</p>
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

/** Custom prop equality for the `memo`-wrapped `ChatExchange` below.
 *
 *  Default `memo` would shallow-compare props, but `exchange` is a fresh
 *  object every `computeExchanges` invocation, so the default would re-render
 *  every child on every SSE event. We compare the **content-relevant**
 *  fingerprint of an Exchange instead:
 *
 *   - `userSeq` — the exchange boundary; switching to a different exchange.
 *   - `steps.length` + last step's `seq` — a new event landed in this
 *     exchange. Events are append-only via `Map.set(seq, …)` with fresh
 *     sequence numbers (never mutated in place — see CLAUDE.md "Events are
 *     immutable, append-only"), so "same length + same last seq" reliably
 *     implies same content.
 *   - `questionOvertaken` — divider exchange flips this when the agent
 *     ignored a question; renders differently.
 *
 *  All other props are primitives or strings — Object.is per field. When
 *  every field matches we return `true` to skip the re-render entirely.
 */
function chatExchangePropsEqual(prev: Props, next: Props): boolean {
  if (prev.streamingBuffer !== next.streamingBuffer) return false;
  if (prev.isLast !== next.isLast) return false;
  if (prev.threadId !== next.threadId) return false;
  if (prev.hasPriorActive !== next.hasPriorActive) return false;
  if (prev.imageOffset !== next.imageOffset) return false;
  if (prev.priorModel !== next.priorModel) return false;
  if (prev.priorEffort !== next.priorEffort) return false;
  if (prev.isUnresumedAbort !== next.isUnresumedAbort) return false;
  if (prev.threadIsCC !== next.threadIsCC) return false;
  if (prev.threadIdle !== next.threadIdle) return false;
  if (prev.threadCanceling !== next.threadCanceling) return false;
  const a = prev.exchange;
  const b = next.exchange;
  if (a.userSeq !== b.userSeq) return false;
  if (a.questionOvertaken !== b.questionOvertaken) return false;
  if (a.steps.length !== b.steps.length) return false;
  const aLast = a.steps[a.steps.length - 1]?.seq;
  const bLast = b.steps[b.steps.length - 1]?.seq;
  if (aLast !== bLast) return false;
  return true;
}

/** Memo-wrapped public component. Drops the 28 unchanged sibling re-renders
 *  on every per-SSE-event ThreadView re-render of the heavy thread. */
export const ChatExchange = memo(ChatExchangeImpl, chatExchangePropsEqual);

/** Hide the response panel's "Canceled ✕" badge when the question card's
 *  own Cancel-as-picked button already carries the same signal. */
export function shouldShowResponseStatusBadge(
  userEventType: ThreadEvent['type'],
  statusClass: string,
): boolean {
  return !(userEventType === 'UserQuestionAsked' && statusClass === 'canceled');
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

type InitiatorVariant = 'user' | 'system' | 'trigger' | 'lucidos';

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
  /** Whether the actor chip opens the route popover. False when the panel
   *  body itself surfaces the same affordance — currently only the
   *  ChildThreadCompleted card, where the title-link replaces the popover's
   *  origin row. Defaults to true. */
  actorClickable?: boolean;
}

/** Action label shared by the panel header and the route popover's Origin row. */
function initiatorSummary(ev: Exchange['userEvent']): string {
  switch (ev.type) {
    case 'TriggerStarted':           return 'Trigger fired';
    case 'ContinuationStarted':         return originMode(ev.actor) === 'human'
      ? 'Continued the response'
      : 'Resumed after engine restart';
    case 'ResponseAborted':            return responseAbortedSummary(ev.actor, ev.cause);
    case 'ResponseCanceled':           return RESPONSE_CANCELED_SUMMARY;
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
    case 'ChildThreadCompleted':         return '';
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
  /** Forwarded to the `UserQuestionAsked` and `CodingAgentPermissionRequest`
   *  arms to disable their buttons. Default `false` so the many existing unit
   *  tests covering unrelated user events don't need to thread it through. */
  responseTerminated: boolean = false,
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
    case 'ContinuationStarted':
      // ContinuationStarted carries an actor (device when triggered by Continue,
      // engine if auto-resume returns). Drive the chip from that actor.
      return {
        variant: actorVariant(ev.actor),
        ...actorInitiator(ev.actor),
        summary,
        details: <ResumeNoteBody exchange={exchange} />,
      };
    case 'ResponseAborted':
      // Exchange boundary — let the actor drive the chip (engine for crashes,
      // device for restarts and user-triggered stale-settle cleanups).
      return {
        variant: actorVariant(ev.actor),
        ...actorInitiator(ev.actor),
        summary,
      };
    case 'ResponseCanceled':
      // ResponseCanceled is an exchange boundary. Cancellation is user-driven
      // on a real in-flight response by definition (CancelCause doc), so
      // default to a 'You' chip when actor is absent — the chat-thread cancel
      // path doesn't yet plumb the actor through. When actor is present (CC
      // cancel paths), let it drive the chip.
      return ev.actor
        ? { variant: actorVariant(ev.actor), ...actorInitiator(ev.actor), summary }
        : youInitiator({ summary });
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
    case 'ChildThreadCompleted':
      return {
        variant: 'lucidos',
        icon: LUCIDOS_AGENT_ICON,
        label: LUCIDOS_AGENT_LABEL,
        actorClickable: false,
        details: (
          <ChildCompletionCard
            childThreadId={ev.child_thread_id}
            childThreadTitle={ev.child_thread_title}
            status={ev.status}
            summary={ev.summary}
          />
        ),
      };
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
            multiSelect={ev.multi_select}
            resolved={answered?.answer}
            terminated={responseTerminated}
          />
        ),
      });
    }
    case 'CodingAgentPermissionRequest': {
      const resolvedStep = findPermissionResolution(exchange, ev.request_id);
      const resolved = resolvedStep
        ? {
            allowed: resolvedStep.allowed,
            reason: resolvedStep.reason,
            persist_scope: resolvedStep.persist_scope,
          }
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
            terminated={responseTerminated}
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

export { describeExecutor } from './chat-exchange-parts';
