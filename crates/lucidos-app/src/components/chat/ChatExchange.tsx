import type { ComponentChildren } from 'preact';
import { memo } from 'preact/compat';
import { useMemo, useRef } from 'preact/hooks';
import { loadedOr } from '../../store/types';
import type { ResponseEvent, App } from '../../store/types';
import type { CodingAgent } from '../../api/types';
import type { Exchange, ThreadEvent, MessageOrigin } from '../../store/thread-events';
import { ENGINE_LABEL, SYSTEM_ICON, SYSTEM_LABEL, API_CALLER_ICON, API_CALLER_LABEL, LUCIDOS_AGENT_LABEL, exchangeUserMessage, exchangeUserImageHashes, exchangeTimestamp, exchangeResponseTimestamp, exchangeResponseText, exchangeEngineLimitDetail, exchangeSteps, exchangeResponseEvents, exchangeStatus, exchangeError, isEmptyContinuedExchange, isCanceledQuestionDivider, changePanelHasContinuation, findCommandPermissionResolution, findMcpPermissionResolution, findPermissionResolution, findQuestionAnswer, isChangeLifecycleEvent, modeToInitiator, originMode, continuationStartedSummary, responseAbortedSummary, RESPONSE_CANCELED_SUMMARY } from '../../store/thread-events';
import { LucidosGlyph } from '../shared/LucidosMark';
import { artifacts, appsList, openImagePopupFromGroup, stepsExpanded, detailsExpanded, collapsedExchanges, toggleExchangeCollapsed, collapsedInitiators, toggleInitiatorCollapsed, toggleMessageRoutePanel } from '../../store/store';
import { removeQueuedMessage } from '../../store/actions/chat';
import { preserveOnToggle } from './scrollState';
import { openFilePreview, openLocalFile } from '../../store/actions/artifacts';
import { openApp } from '../../store/actions/apps';
import { withScrollAnchor } from './CreateThreadView';
import { QuestionBody } from './QuestionCard';
import { CommandPermissionBody, McpPermissionBody, PermissionBody } from './PermissionCard';
import { ChildCompletionCard } from './ChildCompletionCard';
import { getEventToggleState, getCollapsedVisibleEvents, splitEventSections, hasVisibleLiveStep } from '../../store/event-rendering';
import { statusLabel as getStatusLabel, isActive as isStatusActive, isTerminated } from '../../store/exchange-status';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { linkifyPaths, extractAppIdFromHref, extractNavTargetFromHref, extractLocalFileTarget } from '../../utils/linkifyPaths';
import { handleNavigationRequest } from '../../store/actions/thread-sync';
import { ChangeBody, CheckpointCard, ContinueButton, FileList, GeneratedImage, InitiatorPanel, InlineStep, MarkdownBlock, ResponsePanel, ResumeNoteBody, UserMessageBody, changeAccent, changeActions, describeExecutor } from './chat-exchange-parts';
import { TrashIcon } from '../shared/icons';

// Stable refs so loadedOr fallback doesn't yield a fresh [] each render —
// without these, every dependent useMemo invalidates on every render whenever
// artifacts/apps are not loaded.
const NO_ARTIFACTS: string[] = [];
const NO_APPS: App[] = [];

/** The change_id this exchange pertains to, used to stamp `data-change-id` so
 *  the Changes panel can deep-link a row to its originating turn. The aggregate
 *  `ChangeProposed` rides this turn as a (non-rendered) step — read it from
 *  there; lifecycle panels (ChangeApplied/Discarded/Reverted/Failed) carry the
 *  id on the userEvent. Per-commit ChangeProposed emits carry an empty
 *  change_id, so the truthiness check skips them. */
function exchangeChangeId(exchange: Exchange, isChangePanel: boolean, threadIsCC: boolean): string | undefined {
  if (isChangePanel) return (exchange.userEvent as { change_id?: string }).change_id;
  // ChangeProposed only ever rides a coding-agent turn — skip the step scan on
  // chat exchanges entirely.
  if (!threadIsCC) return undefined;
  for (const { event } of exchange.steps) {
    if (event.type === 'ChangeProposed' && event.change_id) return event.change_id;
  }
  return undefined;
}

interface Props {
  exchange: Exchange;
  /** `exchange.revision ?? 0` captured at render time by the parent. The
   *  incremental grouping cache mutates Exchange objects in place, so
   *  `prev.exchange` and `next.exchange` can be the SAME object — field
   *  compares through it are self-comparisons. This primitive is the
   *  mutation signal the memo can actually see. */
  revision: number;
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
  /** True when this is a chat follow-up the user typed while the agent was busy
   *  — it's queued behind the active turn and not yet ingested. Computed in
   *  `renderExchanges` (it needs thread-level busy state + the active-exchange
   *  index); drives the "Queued" marker on the bubble. */
  isQueued?: boolean;
  /** Lifted from `threadMap.value.get(threadId)?.meta.channel === 'claude_code'`
   *  in `renderExchanges` so this component does not subscribe to threadMap
   *  itself — see `chatExchangePropsEqual` below for the memo contract. */
  threadIsCC: boolean;
  /** Specific coding-agent backend for labels/icons. The channel remains
   *  `claude_code` for both Claude Code and Codex for backwards compatibility. */
  threadCodingAgent: CodingAgent;
  /** Lifted from `isRenderedThreadIdle(threadMap.value.get(threadId))` — quiescent
   *  by raw status, but false while an optimistic resume is in flight. */
  threadIdle: boolean;
  /** Lifted from `threadMap.value.get(threadId)?.meta.status === 'waiting_for_user_answer'`.
   *  Tells `exchangeStatus` the thread is parked on / resuming from a question or
   *  permission card — never crashed — so a just-answered divider doesn't flash
   *  "Aborted" during the answer→resume gap. See `exchangeStatus`. */
  threadAwaitingAnswer: boolean;
  /** Lifted from `cancelingThreadIds.value.has(threadId)`. */
  threadCanceling: boolean;
  /** First line of the in-thread `ChangeProposed` description + its file count
   *  for this exchange's change_id (built once per thread in `renderExchanges`).
   *  Seeds <ChangeBody> so a change-lifecycle panel paints at its final height
   *  on first open, before the per-id `Change` lazy-fetch lands — fixing the
   *  open-path jump. Undefined for non-change exchanges (and when no matching
   *  ChangeProposed rode the thread). Primitives, so the memo stays cheap. */
  proposedChangeDesc?: string;
  proposedChangeFileCount?: number;
}

function ChatExchangeImpl({ exchange, streamingBuffer, isLast, isQueued, threadId, hasPriorActive, imageOffset = 0, priorModel, priorEffort, isUnresumedAbort, threadIsCC, threadCodingAgent, threadIdle, threadAwaitingAnswer, threadCanceling, proposedChangeDesc, proposedChangeFileCount }: Props) {
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
  const status = exchangeStatus(exchange, streamingBuffer, isLast, hasPriorActive, threadIsCC, threadIdle, threadAwaitingAnswer);
  const error = exchangeError(exchange);

  // Cap detection reads ResponseGenerated.text directly via exchangeEngineLimitDetail —
  // the cap is emitted without a preceding TextStreamed, so it never lands in
  // responseTextRaw (which only concatenates streamed text). Without this side
  // channel the agent appears to stop silently mid-task.
  const engineLimitDetail = !streamingBuffer ? exchangeEngineLimitDetail(exchange) : '';
  const isEngineLimit = !!engineLimitDetail;
  // The live streaming buffer changes every token — opt it out of the markdown
  // cache so its short-lived fragments don't evict the stable, reused entries.
  const streamingHtml = streamingBuffer ? renderMarkdown(streamingBuffer, { cache: false }) : '';
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
      // A `file://` URL or an absolute filesystem path (a staged release .dmg
      // under ~/…/.lucidos/release-worktrees/, an /Applications/… folder, …) —
      // open it with the OS (mount the dmg / reveal the folder), NOT via the
      // in-app file preview or the /data/* static mount (those are for
      // workspace-relative paths only). Runs AFTER the app/nav extractors so
      // their absolute routes (/apps/<id>/…, /notifications, …) keep working;
      // http(s) URLs return null here and keep their browser/panel behavior.
      const localFile = extractLocalFileTarget(rawHref);
      if (localFile) {
        e.preventDefault();
        openLocalFile(localFile);
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

  // Sections (split on section_break) tagged with each section's base index in
  // visibleEvents, so `renderResponseEvents` can key rows by a stable index even
  // when the list grows during streaming. `splitEventSections` drops the break
  // markers, so re-walk `visibleEvents` to recover each section's offset (the
  // while-loop handles consecutive breaks). The whole exchange renders — the open
  // cost is bounded at the LOAD layer (the snapshot tail window) instead, so a
  // huge coding-agent turn only ever holds a tail of its events.
  const renderedSections = useMemo(() => {
    const sections = splitEventSections(visibleEvents);
    let cursor = 0;
    return sections.map((events) => {
      while (cursor < visibleEvents.length && visibleEvents[cursor].type === 'section_break') cursor++;
      const base = cursor;
      cursor += events.length;
      return { events, base };
    });
  }, [visibleEvents]);

  // Exactly one running-text shimmer at a time: if an in-progress step is
  // visible on screen, its own shimmer is the "live" affordance, so the
  // "Working" status label below drops to a plain, static label. Otherwise —
  // steps hidden, this exchange collapsed (steps body hidden), or expanded with
  // no pending step — the label shimmers as the sole affordance.
  const liveStepOnScreen = hasVisibleLiveStep(showSteps, isCollapsed, visibleEvents);

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
    // The streaming buffer's html changes every token — opt its linkify out of
    // the LRU cache (mirrors renderMarkdown's cache:false for the same buffer
    // above) so per-token html doesn't thrash the cache and evict stable entries.
    () => linkifyPaths(responseHtmlCombined, artifactPaths, apps, { cache: !streamingBuffer }),
    [responseHtmlCombined, artifactPaths, apps, streamingBuffer],
  );

  const responseTerminated = isTerminated(status) || exchange.questionOvertaken === true;

  const initiator = useMemo(
    () => describeInitiator(exchange, userMessageHtml, userImageHashes, threadId, responseTerminated, threadIsCC, threadCodingAgent, proposedChangeDesc, proposedChangeFileCount),
    [exchange, userMessageHtml, userImageHashes, threadId, responseTerminated, threadIsCC, threadCodingAgent, proposedChangeDesc, proposedChangeFileCount],
  );
  const canCollapseInitiator = !!initiator.summary || !!initiator.details;
  const isInitiatorCollapsed = canCollapseInitiator
    && collapsedInitiators.value.has(`${threadId}:${exchange.userSeq}`);
  const isChangePanel = isChangeLifecycleEvent(exchange.userEvent);
  const changeId = exchangeChangeId(exchange, isChangePanel, threadIsCC);
  // Card-less treatment: a human chat message renders as a right-aligned
  // accent-tinted bubble; change-lifecycle turns render flat. Both drop the
  // actor chip (icon + "You"/name) — attribution moves to the clickable
  // timestamp/summary. Predicate is event-type based, NOT label-based:
  // user-driven control turns (cancel/restart/continue/auto-prompt/credential/
  // consent) keep the chip slot but render it iconless with the action AS the
  // label (see actionInitiator), and question/permission dividers keep their
  // agent chip.
  const isUserMessageBubble = exchange.userEvent.type === 'MessageReceived' && initiator.variant === 'user';
  // A queued follow-up (typed while the agent was busy, not yet ingested) shows
  // a "Queued" tag in its own bubble header — where dividers show "Answered
  // ✓" — instead of a faux "Lucidos Agent — Queued" response panel below it.
  // The message is the user's, and a stack of them should each read as waiting.
  const isQueuedUserMessage = !!isQueued && isUserMessageBubble;
  const queuedMessageId = isQueuedUserMessage ? exchange.userEvent._eventId : undefined;
  // The trash button lives INSIDE the status label (an existing `display:flex`
  // row) rather than in a separate wrapper, so "Queued" and the trash stay on
  // one line using only CSS that already ships — no dependency on a fresh rule.
  const queuedStatus = (
    <span class="exchange-status-label exchange-status-queued">
      {'Queued'}
      {queuedMessageId && (
        <button
          type="button"
          class="icon-btn queued-message-remove"
          aria-label="Remove queued message"
          data-tooltip="Remove queued message"
          onClick={(e) => {
            e.stopPropagation();
            void removeQueuedMessage(threadId, queuedMessageId);
          }}
        >
          <TrashIcon />
        </button>
      )}
    </span>
  );
  const isChromeless = isUserMessageBubble || isChangePanel;
  const isAbortPanel = exchange.userEvent.type === 'ResponseAborted';
  const isCancelPanel = exchange.userEvent.type === 'ResponseCanceled';
  const isCanceledDivider = isCanceledQuestionDivider(exchange);
  // Change lifecycle, abort-boundary, cancel-boundary, and canceled-question-
  // divider exchanges are terminal — they have no response, just the initiator
  // panel with optional actions (Diff/Revert on change panels, Continue on the
  // unresumed abort, the QuestionCard's own ✓ Cancel button on the divider).
  // Exception: a change banner whose session KEPT WORKING after the apply
  // (no new user message → the continuation folded into this exchange as steps)
  // must render its body, or that work + its follow-up proposal are invisible
  // between two "Change applied" rows (real thread 76b4ee76).
  const isChangeContinuation = isChangePanel && changePanelHasContinuation(exchange);
  const showResponsePanel = (!isChangePanel || isChangeContinuation) && !isAbortPanel && !isCancelPanel && !isCanceledDivider && !isEmptyContinued && !isQueuedUserMessage && (hasResponse || hasEvents || showStatus);
  let initiatorActions: ComponentChildren | undefined;
  if (isChangePanel) {
    initiatorActions = changeActions(
      (exchange.userEvent as { change_id?: string }).change_id,
      exchange.userEvent.type === 'ChangeApplyFailed',
      // ChangeApplied always resolves to at least a Revert button — reserve the
      // footer row while the Change row loads so the buttons don't shift the
      // panel down on first open (mirrors the body's ChangeProposed seed).
      exchange.userEvent.type === 'ChangeApplied',
    );
  } else if (isAbortPanel && isUnresumedAbort) {
    initiatorActions = <ContinueButton threadId={threadId} />;
  }
  const executor = describeExecutor(threadIsCC, threadCodingAgent);

  function renderResponseEvents(eventsList: ResponseEvent[], baseIndex = 0) {
    return eventsList.map((evt, i) => {
      // Key by ABSOLUTE index in visibleEvents (baseIndex + i), not the local
      // per-section slice index: `splitEventSections` hands each section its base
      // offset, so a row's key is stable across renders even as earlier sections
      // grow during streaming — which keeps the visible rows stable instead of
      // re-rendering the whole tail (and re-setting every
      // `dangerouslySetInnerHTML`) on each streamed event.
      const k = baseIndex + i;
      if (evt.type === 'text' && evt.md?.trim()) {
        return <div key={`t${k}`} dangerouslySetInnerHTML={{ __html: visibleTextHtmls.get(evt)! }} />;
      }
      if (evt.type === 'step' && showSteps) return <InlineStep key={`s${k}`} event={evt} />;
      if (evt.type === 'image') return <GeneratedImage key={`img${k}`} event={evt} />;
      if (evt.type === 'checkpoint') return <CheckpointCard key={`cp${k}`} event={evt} />;
      if (evt.type === 'empty') return <div key={`e${k}`} class="response-empty-note">{'The model returned an empty response.'}</div>;
      return null;
    });
  }

  return (
    <div class="chat-exchange" ref={rootRef} data-event-id={exchange.userEvent._eventId} data-change-id={changeId || undefined}>
      <InitiatorPanel
        initiator={isQueuedUserMessage
          ? { ...initiator, status: queuedStatus }
          : initiator}
        timestamp={formatMessageTimestamp(timestamp)}
        onActorClick={initiator.actorClickable === false
          ? undefined
          : (e) => openInfoPanel('origin', e)}
        actions={initiatorActions}
        bubble={isUserMessageBubble}
        chromeless={isChromeless}
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
              {/* The active status label — Working / Requesting / Canceling —
                  shimmers as the AI running-text affordance, which replaces the
                  spinner (no mini-spinner in the 'working' state). Suppressed
                  when a live step is already shimmering on screen, so only one
                  running-text affordance moves at a time (see liveStepOnScreen). */}
              <span class={statusClass === 'working' && !liveStepOnScreen ? 'running-shimmer' : undefined}>{statusLabelText}</span>
              {statusClass === 'queued' && <span class="exchange-status-queued">{'○'}</span>}
              {statusClass === 'waiting' && <span class="progress-dot progress-dot-waiting" />}
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
            renderedSections.map(({ events: section, base }, sIdx) => (
              <div class="response-content markdown-content" key={`sec-${base}`} onClick={handleLinkClick}>
                {sIdx === 0 && renderToggles()}
                {renderResponseEvents(section, base)}
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
 *  Default `memo` would shallow-compare props; a from-scratch
 *  `computeExchanges` pass produces fresh Exchange objects, so the default
 *  would re-render every child on every SSE event. We compare a
 *  **content-relevant** fingerprint instead:
 *
 *   - `revision` — the in-place mutation counter, captured as a primitive at
 *     render time. The incremental grouping cache keeps Exchange objects
 *     identity-stable and mutates them in place (steps push,
 *     questionOvertaken flip), so when `prev.exchange === next.exchange`
 *     every field compare below is a self-comparison and detects nothing —
 *     the captured revision numbers are the only honest change signal.
 *   - `userSeq` — the exchange boundary; switching to a different exchange.
 *   - `steps.length` + last step's `seq` — a new event landed in this
 *     exchange (covers the fresh-objects case where identities differ).
 *   - `questionOvertaken` — divider exchange flips this when the agent
 *     ignored a question; renders differently.
 *
 *  All other props are primitives or strings — Object.is per field. When
 *  every field matches we return `true` to skip the re-render entirely.
 */
function chatExchangePropsEqual(prev: Props, next: Props): boolean {
  if (prev.revision !== next.revision) return false;
  if (prev.streamingBuffer !== next.streamingBuffer) return false;
  if (prev.isLast !== next.isLast) return false;
  if (prev.isQueued !== next.isQueued) return false;
  if (prev.threadId !== next.threadId) return false;
  if (prev.hasPriorActive !== next.hasPriorActive) return false;
  if (prev.imageOffset !== next.imageOffset) return false;
  if (prev.priorModel !== next.priorModel) return false;
  if (prev.priorEffort !== next.priorEffort) return false;
  if (prev.isUnresumedAbort !== next.isUnresumedAbort) return false;
  if (prev.threadIsCC !== next.threadIsCC) return false;
  if (prev.threadCodingAgent !== next.threadCodingAgent) return false;
  if (prev.threadIdle !== next.threadIdle) return false;
  if (prev.threadAwaitingAnswer !== next.threadAwaitingAnswer) return false;
  if (prev.threadCanceling !== next.threadCanceling) return false;
  if (prev.proposedChangeDesc !== next.proposedChangeDesc) return false;
  if (prev.proposedChangeFileCount !== next.proposedChangeFileCount) return false;
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
  /** Icon glyph (emoji string) or a component (e.g. the Claude logo for a
   *  question asked inside a coding-agent thread). */
  icon: ComponentChildren;
  /** WHO performed this — always the initiator's display name. */
  label: string;
  /** Optional resolution status shown in the header (question/permission
   *  dividers: "Answered" / "Needs your answer" / "Canceled"). */
  status?: ComponentChildren;
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
    case 'ContinuationStarted':         return continuationStartedSummary(ev.reason, ev.actor);
    case 'ResponseAborted':            return responseAbortedSummary(ev.actor, ev.cause);
    // ResponseCanceled carries its text as the header label (RESPONSE_CANCELED_SUMMARY),
    // not as a summary line — see its describeInitiator arm.
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

/** Map a `MessageOrigin` to its display icon + label. The chip answers "who
 *  decided this" with one of the closed set of known actors: **You** (a real
 *  browser device), **Lucidos Agent** (LLM acting on behalf of the user —
 *  rendered as the Lucidos mark), **Lucidos Engine** (deterministic engine
 *  work — rendered as the SAME Lucidos mark; the label is what tells it apart
 *  from the agent), **System** (host killed the process), or **API caller**
 *  (external HTTP caller that did NOT self-identify as one of the above).
 *
 *  "You" is reserved for `kind: device` — a browser session bound to a known
 *  device row. Any other human-mode origin (anonymous API client, cross-workspace
 *  human, …) renders as "API caller" so an unauthenticated POST can never
 *  impersonate the user in the timeline. The popover still discloses the
 *  origin kind, user-agent, and workspace name underneath.
 *
 *  Lives in the view layer (not the store) because both Lucidos actor icons are
 *  a brand component (`<LucidosGlyph/>`), matching how `describeExecutor`
 *  resolves the same glyph — the store model stays free of UI components. */
export function actorInitiator(actor: MessageOrigin | undefined): { icon: ComponentChildren; label: string } {
  if (actor?.kind === 'system') return { icon: SYSTEM_ICON, label: SYSTEM_LABEL };
  if (actor?.kind === 'device') return { icon: '\u{1F464}', label: 'You' };
  switch (originMode(actor)) {
    case 'human':  return { icon: API_CALLER_ICON, label: API_CALLER_LABEL };
    case 'agent':  return { icon: <LucidosGlyph />, label: LUCIDOS_AGENT_LABEL };
    case 'engine': return { icon: <LucidosGlyph />, label: ENGINE_LABEL };
  }
}

/** Build a `'user'`-variant initiator descriptor with the standard human chip
 *  (icon + "You" label) and a caller-supplied summary/details/accent. Shared by
 *  every arm where the device-owner is the initiator (MessageReceived from a
 *  device, divider-starter ActionRequired events, …). */
function youInitiator(rest: Partial<InitiatorDescriptor> = {}): InitiatorDescriptor {
  return { variant: 'user', icon: '\u{1F464}', label: 'You', ...rest };
}

/** Build a `'system'`-variant descriptor with the engine chip (Lucidos mark +
 *  Lucidos Engine). Shared by every arm where the engine narrates its own action
 *  (hardening / merge-conflict detection, legacy bare CC prompt). */
function engineInitiator(summary: string, details?: ComponentChildren): InitiatorDescriptor {
  return { variant: 'system', icon: <LucidosGlyph />, label: ENGINE_LABEL, summary, details };
}

/** Build a descriptor in the "Response canceled" style: no icon, the action
 *  text AS the label, and no separate summary line. The label chip stays
 *  clickable (it opens the origin popover, which discloses who/what — "You",
 *  the device, "Lucidos credential request", …). Shared by every user-driven
 *  control turn (Restart, Continue, auto-prompt, credential/consent) so they
 *  read as clean boundaries, matching the ResponseCanceled turn. `details`
 *  carries any richer body (resume note, injected prompt). */
function actionInitiator(label: string, details?: ComponentChildren): InitiatorDescriptor {
  return { variant: 'system', icon: null, label, details };
}

/** Which terminal verdict a divider header carries when the prompt was never
 *  resolved: `'canceled'` only when the USER explicitly dismissed it, `'dropped'`
 *  for every other turn-ended-without-a-response cause (system abort, error, the
 *  agent racing past the prompt). */
type DividerTerminalKind = 'canceled' | 'dropped';

/** Resolution status for question / permission dividers — shown in the initiator
 *  header. The header describes what happened to the PROMPT, never the turn:
 *  "Answered"/"Resolved" (✓) when the user responded, "Canceled" (✕) when the
 *  user explicitly dismissed it, "Unanswered"/"Unresolved" (muted) when the turn
 *  ended without a response for any other reason, and a plain "Needs your answer"
 *  call-to-action while pending. The turn's own terminal cause (Aborted ⚠ /
 *  Error ✕) is carried by the response panel + the abort boundary — the header
 *  must NOT impersonate it (a system abort previously rendered here as the
 *  user-driven "Canceled", contradicting the "Aborted" panel right below it).
 *  Reuses the response panel's `.exchange-status-*` classes so the glyphs match. */
function dividerStatus(
  resolved: boolean,
  resolvedLabel: string,
  droppedLabel: string,
  terminal: DividerTerminalKind | null,
): ComponentChildren {
  if (resolved) return <span class="exchange-status-label exchange-status-done">{resolvedLabel}<span class="exchange-status-check">{'✓'}</span></span>;
  if (terminal === 'canceled') return <span class="exchange-status-label exchange-status-canceled">{'Canceled'}<span class="exchange-status-x">{'✕'}</span></span>;
  if (terminal === 'dropped') return <span class="exchange-status-label exchange-status-dropped">{droppedLabel}</span>;
  return <span class="exchange-status-label exchange-status-awaiting">{'Needs your answer'}</span>;
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
  /** Whether this is a coding-agent thread — picks the asking agent's chip
   *  (specific coding agent vs Lucidos Agent) for question/permission
   *  dividers. */
  threadIsCC: boolean = false,
  threadCodingAgent: CodingAgent = 'claude-code',
  /** First line of the in-thread `ChangeProposed` description + its file count,
   *  forwarded to <ChangeBody> for the change-lifecycle arms so the body paints
   *  at full height on first open (before the per-id Change fetch lands). */
  proposedChangeDesc?: string,
  proposedChangeFileCount?: number,
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
      // engine if auto-resume returns). A device-driven continue is a user
      // action → render it in the iconless ResponseCanceled style (action AS the
      // label); engine auto-resume keeps the Lucidos-mark chip.
      if (ev.actor?.kind === 'device') {
        return actionInitiator(summary, <ResumeNoteBody exchange={exchange} />);
      }
      return {
        variant: actorVariant(ev.actor),
        ...actorInitiator(ev.actor),
        summary,
        details: <ResumeNoteBody exchange={exchange} />,
      };
    case 'ResponseAborted':
      // Exchange boundary — let the actor drive the chip (engine for crashes,
      // device for restarts and user-triggered stale-settle cleanups). A
      // device-driven abort (you hit Restart) renders iconless like a cancel;
      // engine/system aborts keep their Lucidos-mark/⚙ chip.
      if (ev.actor?.kind === 'device') {
        return actionInitiator(summary);
      }
      return {
        variant: actorVariant(ev.actor),
        ...actorInitiator(ev.actor),
        summary,
      };
    case 'ResponseCanceled':
      // ResponseCanceled is an exchange boundary, always user-driven by
      // definition (CancelCause doc). It is the archetype for the iconless
      // boundary style (see actionInitiator): "Response canceled" IS the header
      // label, and clicking it opens the Initiator info popover (which discloses
      // "You", the device, and the cancel cause).
      return actionInitiator(RESPONSE_CANCELED_SUMMARY);
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
        details: <ChangeBody changeId={ev.change_id} seedDescription={proposedChangeDesc} seedFileCount={proposedChangeFileCount} />,
      };
    case 'ChangeApplyFailed':
      return {
        variant: 'system', accent: 'change-failed',
        ...actorInitiator(ev.actor),
        summary,
        details: <ChangeBody changeId={ev.change_id} error={ev.error} seedDescription={proposedChangeDesc} seedFileCount={proposedChangeFileCount} />,
      };
    case 'UserPromptInjected':
      // Legacy rows lack `origin` and fall back to the engine label. A
      // device-origin injection (you re-prompted) renders iconless like a
      // cancel, keeping the injected prompt as the body.
      if (ev.origin?.kind === 'device') {
        return actionInitiator(summary, <MarkdownBlock html={userMessageHtml} />);
      }
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
      // The EventBus fan-in path raises this on the parent when a child thread
      // reaches a terminal event — deterministic engine plumbing, not LLM work
      // — so attribute it to the engine (Lucidos mark + Lucidos Engine),
      // consistent with every other engine-injected event (trigger fired,
      // hardening, merge). The child agent's authored summary lives in the card
      // body; the chip is non-clickable because the title-link is the origin
      // affordance.
      return {
        variant: 'system',
        icon: <LucidosGlyph />,
        label: ENGINE_LABEL,
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
      // The agent ASKS the question; attribute the divider to it (Lucidos Agent
      // or Claude Code), with a resolution status in the header. Resolution
      // lives on this exchange's steps as UserQuestionAnswered; matched by
      // tool_use_id so a stale Answered from a different question can't bleed in.
      const answered = findQuestionAnswer(exchange, ev.tool_use_id);
      // A canceled question resolves via a UserQuestionAnswered{kind:'Canceled'},
      // which findQuestionAnswer still returns — so exclude it from "Answered"
      // and route it to the "Canceled" status instead.
      const canceled = isCanceledQuestionDivider(exchange);
      const agent = describeExecutor(threadIsCC, threadCodingAgent);
      return {
        variant: 'lucidos',
        icon: agent.icon,
        label: agent.label,
        status: dividerStatus(
          !!answered && !canceled,
          'Answered',
          'Unanswered',
          canceled ? 'canceled' : responseTerminated ? 'dropped' : null,
        ),
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
      };
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
      const agent = describeExecutor(true, threadCodingAgent);
      return {
        variant: 'lucidos',
        icon: agent.icon,
        label: agent.label,
        status: dividerStatus(!!resolvedStep, 'Resolved', 'Unresolved', responseTerminated ? 'dropped' : null),
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
      };
    }
    case 'CommandPermissionRequested': {
      const resolvedStep = findCommandPermissionResolution(exchange, ev.request_id);
      const resolved = resolvedStep
        ? {
            allowed: resolvedStep.allowed,
            reason: resolvedStep.reason,
            persist_scope: resolvedStep.persist_scope,
          }
        : undefined;
      // The command guard only fires on chat threads → the Lucidos Agent.
      const agent = describeExecutor(false);
      return {
        variant: 'lucidos',
        icon: agent.icon,
        label: agent.label,
        status: dividerStatus(!!resolvedStep, 'Resolved', 'Unresolved', responseTerminated ? 'dropped' : null),
        details: (
          <CommandPermissionBody
            event={{
              request_id: ev.request_id,
              tool_use_id: ev.tool_use_id,
              tool_name: ev.tool_name,
              command: ev.command,
              summary: ev.summary,
            }}
            resolved={resolved}
            terminated={responseTerminated}
          />
        ),
      };
    }
    case 'McpPermissionRequested': {
      const resolvedStep = findMcpPermissionResolution(exchange, ev.request_id);
      const resolved = resolvedStep
        ? {
            allowed: resolvedStep.allowed,
            reason: resolvedStep.reason,
            persist_scope: resolvedStep.persist_scope,
          }
        : undefined;
      // The chat MCP permission lane only fires on chat threads → Lucidos Agent.
      const agent = describeExecutor(false);
      return {
        variant: 'lucidos',
        icon: agent.icon,
        label: agent.label,
        status: dividerStatus(!!resolvedStep, 'Resolved', 'Unresolved', responseTerminated ? 'dropped' : null),
        details: (
          <McpPermissionBody
            event={{
              request_id: ev.request_id,
              tool_use_id: ev.tool_use_id,
              server_id: ev.server_id,
              server_name: ev.server_name,
              tool_name: ev.tool_name,
              arguments_summary: ev.arguments_summary,
            }}
            resolved={resolved}
            terminated={responseTerminated}
          />
        ),
      };
    }
    case 'CredentialRequested':
    case 'McpConsentRequested':
      // Iconless action label (ResponseCanceled style); the asker — "Lucidos
      // credential request" — is disclosed in the timestamp popover. No body
      // component today; the engine surfaces these via separate transient flows.
      return actionInitiator(summary);
    default:
      // Unreachable in production (groupIntoExchanges only assigns starter
      // types to userEvent), but `userEvent: StoredEvent` covers every event
      // variant for legacy reasons, so TS can't enforce exhaustiveness here.
      return youInitiator();
  }
}

export { describeExecutor } from './chat-exchange-parts';
