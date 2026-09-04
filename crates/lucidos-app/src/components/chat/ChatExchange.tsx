import type { ComponentChildren } from 'preact';
import type { Signal } from '@preact/signals';
import { memo } from 'preact/compat';
import { useEffect, useMemo, useState } from 'preact/hooks';
import { loadedOr } from '../../store/types';
import type { ResponseEvent, App } from '../../store/types';
import type { CodingAgent } from '../../api/types';
import type { Exchange, ThreadEvent, MessageOrigin } from '../../store/thread-events';
import { ENGINE_LABEL, SYSTEM_LABEL, API_CALLER_LABEL, LUCIDOS_AGENT_LABEL, abortPromisesAutoResume, exchangeUserMessage, exchangeUserImageHashes, exchangeTimestamp, exchangeResponseTimestamp, exchangeResponseText, exchangeEngineLimitDetail, exchangeSteps, exchangeResponseEvents, exchangeStatus, exchangeError, dividerBodyIsSuppressed, hasRenderableResponseContent, isEmptyContinuedExchange, questionDividerResolution, changePanelHasContinuation, findCommandPermissionResolution, findMcpPermissionResolution, findPermissionResolution, findQuestionAnswer, isChangeLifecycleEvent, isLiveUtteranceRow, modeToInitiator, originMode, continuationStartedSummary, responseAbortedSummary, eventWaitStoppedSummary, isUserStoppedWait, RESPONSE_CANCELED_SUMMARY } from '../../store/thread-events';
import { LucidosGlyph } from '../shared/LucidosMark';
import { artifacts, appsList, openImagePopupFromGroup, showToast, stepsExpanded, detailsExpanded, collapsedExchanges, toggleExchangeCollapsed, expandExchange, collapsedInitiators, toggleInitiatorCollapsed, toggleMessageRoutePanel } from '../../store/store';
import { removeQueuedMessage } from '../../store/actions/chat';
import { openFilePreview, openLocalFile } from '../../store/actions/artifacts';
import { openApp, openAppById } from '../../store/actions/apps';
import { withScrollAnchor } from './CreateThreadView';
import { QuestionBody } from './QuestionCard';
import { CommandPermissionBody, McpPermissionBody, PermissionBody } from './PermissionCard';
import { ChildCompletionRow } from './ChildCompletionRow';
import { hidesEarlierProse, getCollapsedVisibleEvents, splitEventSections, liveStepIndex, drawsResponseRow } from '../../store/event-rendering';
import { statusLabel as getStatusLabel, isActive as isStatusActive, isTerminated, type ExchangeStatus } from '../../store/exchange-status';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { linkifyPaths, extractAppTargetFromHref, extractNavTargetFromHref, extractLocalFileTarget, extractBareAppRef, extractDataPathTarget, extractTriggerIdFromHref, hasUrlScheme, browserHandlesHref } from '../../utils/linkifyPaths';
import { handleNavigationRequest } from '../../store/actions/thread-sync';
import { navigateToTrigger } from '../../store/actions/triggers';
import { ChangeBody, CheckpointCard, ContinueButton, EventDeliveryBody, EventWaitRow, FileList, GeneratedImage, InitiatorPanel, InlineStep, LiveUtteranceBody, MarkdownBlock, ResponsePanel, ResumeNoteBody, SpokenChip, SpokenReply, TriggerFiredBody, UserMessageBody, changeAccent, changeActions, describeExecutor, turnControls } from './chat-exchange-parts';
import { TrashIcon, PowerIcon, PersonIcon, ApiPlugIcon, TriggerFiredIcon } from '../shared/icons';
import { setThreadLive } from './scrollState';
import { useOnScreenInTranscript } from '../../hooks/useOnScreenInTranscript';

// Stable refs so the `loadedOr` fallback does not yield a fresh [] each render.
// Without these, every dependent useMemo invalidates on every render while
// artifacts or apps are not loaded.
const NO_ARTIFACTS: string[] = [];
const NO_APPS: App[] = [];

/** Toast key for the terminal dead-link guard in `handleLinkClick`, so tapping
 *  the same dead link repeatedly replaces the toast rather than stacking. */
const DEAD_LINK_TOAST_KEY = 'chat-dead-link';

/** What the terminal guard says about an href it swallowed. Three cases, named
 *  apart because they have different fixes. An unopenable SCHEME is usually one
 *  the agent invented. An unresolved relative href usually names something that
 *  moved. An empty one cannot be quoted at all. */
function deadLinkMessage(href: string): string {
  if (!href) return 'This link has no destination';
  if (hasUrlScheme(href)) return `Link "${href}" uses a scheme nothing here can open`;
  return `Link "${href}" points nowhere in this workspace`;
}

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
  priorModel?: string;
  priorEffort?: string;
  /** True when this exchange is the abort the user may resume from, the only
   *  one that shows the Continue button. See `continuableAbortIndex`: a
   *  switch-teardown abort the engine is auto-resuming is deliberately not
   *  continuable. */
  isContinuableAbort?: boolean;
  /** True when this is a chat follow-up typed while the agent was busy, queued
   *  behind the active turn and not yet ingested. Computed in `renderExchanges`,
   *  which has the thread-level busy state and the active-exchange index. Drives
   *  the "Queued" marker on the bubble. */
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
   *  Tells `exchangeStatus` the thread is parked on a question or permission
   *  card rather than crashed. A just-answered divider must not flash "Aborted"
   *  during the answer-to-resume gap. */
  threadAwaitingAnswer: boolean;
  /** Lifted from `cancelingThreadIds.value.has(threadId)`. */
  threadCanceling: boolean;
  /** How many of this turn's leading rows the render window leaves out.
   *  Non-zero only on the oldest turn on screen (`threadWindow.ts`), and only
   *  while the reader has not scrolled up into it.
   *
   *  A coding-agent turn can hold 684 rows against a whole-transcript budget of
   *  160, so gating whole turns left the transcript rendering one turn of
   *  everything. This clamps the head of that one turn.
   *
   *  There is NO control that reveals it, deliberately. A per-turn "Show
   *  earlier steps" expander shipped twice and was removed twice, the second
   *  time because the user disliked it from the first message. The head arrives
   *  by scrolling, through the same expansion older turns arrive through. */
  rowsHidden?: number;
  /** First line of the in-thread `ChangeProposed` description + its file count
   *  for this exchange's change_id (built once per thread in `renderExchanges`).
   *  Seeds <ChangeBody> so a change-lifecycle panel paints at its final height
   *  on first open, before the per-id `Change` lazy-fetch lands — fixing the
   *  open-path jump. Undefined for non-change exchanges (and when no matching
   *  ChangeProposed rode the thread). Primitives, so the memo stays cheap. */
  proposedChangeDesc?: string;
  proposedChangeFileCount?: number;
  /** The event a *thread subscription* delivered, resolved once per thread in
   *  `renderExchanges` by following this exchange's
   *  `UserPromptInjected.delivered_event_id`. Resolved THERE because the target
   *  `EventWaitDelivered` sits in a different exchange. Reading `threadMap` from
   *  this component would undo the memo keeping a 29-exchange thread from
   *  re-parsing every markdown body per SSE event. All undefined unless this
   *  exchange is such a delivery. */
  matchedEventType?: string;
  matchedEventId?: string;
  matchedPayloadJson?: string;
}

/** Which boundaries the reader owns, and so draw the right-aligned bubble.
 *
 *  Two events, one act. A caller's utterance is a `SpokenMessageReceived` when
 *  the talker answered it alone and a `MessageReceived` when it delegated. The
 *  reader said the same thing either way, so both read the same way.
 *
 *  Exported for the test that pins that, since the bubble decision itself lives
 *  inside a component with no jsdom to mount it in. */
export function isUserBubbleEvent(userEvent: { type: string }): boolean {
  return userEvent.type === 'MessageReceived' || userEvent.type === 'SpokenMessageReceived';
}

/** Does this exchange mean the AGENT IS RUNNING on the thread being shown? The
 *  follow's live term (see `setThreadLive` in `scrollState`), and nothing else,
 *  so it can afford to be strict: it decides whether the reader's scroll means
 *  "stop dragging me" or merely "I am browsing" (ADR 0064).
 *
 *  TWO sources have to agree, and the second is load-bearing. The exchange
 *  status alone is a RENDERING verdict about one turn, and its final
 *  fallthrough is `'pending'`. A stepless SYSTEM boundary reaches that line
 *  too. `ChangeApplied` opens an exchange of its own, so a coding-agent thread
 *  whose change was applied ends with no steps and no terminal. `'pending'` is
 *  in `ACTIVE_STATUSES` and nothing paints it, so the follow would believe such
 *  a thread was live forever.
 *
 *  So ask the THREAD PROJECTION as well. `threadIdle` is the aggregate's own
 *  "this thread is quiescent", which is the exact question and the only
 *  authority on it. Parked on a question counts as idle, correctly.
 *
 *  Deliberately NOT fixed in `exchangeStatus`. Its `'pending'` fallthrough is
 *  load-bearing, and every status label in the app hangs off that function. */
export function exchangeMarksThreadLive(
  isLast: boolean,
  status: ExchangeStatus,
  threadIdle: boolean,
): boolean {
  return isLast && isStatusActive(status) && !threadIdle;
}

/** Run `fn` with the control the reader pressed pinned exactly where it is.
 *
 *  The anchor is `currentTarget`, the element carrying this handler, so it IS
 *  the control. A turn control only ever changes heights. The reader asked for
 *  that by pressing one named thing, so that thing is what must not move. See
 *  `withScrollAnchor`.
 *
 *  Read before `fn` runs, because the mutation can take the node away: the `⋯`
 *  stub is replaced by the body it reveals. `withScrollAnchor` then writes
 *  nothing, which is exact for that press. An unfold changes nothing above its
 *  own turn, so the freeze has already left the reader where they belong. */
function heldOnThePress(fn: () => void): (e: MouseEvent) => void {
  return (e) => withScrollAnchor(e.currentTarget as HTMLElement | null, fn);
}

function ChatExchangeImpl({ exchange, streamingBuffer, isLast, isQueued, threadId, hasPriorActive, priorModel, priorEffort, isContinuableAbort, threadIsCC, threadCodingAgent, threadIdle, threadAwaitingAnswer, threadCanceling, rowsHidden = 0, proposedChangeDesc, proposedChangeFileCount, matchedEventType, matchedEventId, matchedPayloadJson }: Props) {
  const showDetails = detailsExpanded.value;
  const showSteps = stepsExpanded.value;
  const artifactPaths = loadedOr(artifacts.value, NO_ARTIFACTS);
  const apps = loadedOr(appsList.value, NO_APPS);

  const userMessage = exchangeUserMessage(exchange);
  const userImageHashes = exchangeUserImageHashes(exchange);
  const timestamp = exchangeTimestamp(exchange);
  const responseTextRaw = exchangeResponseText(exchange);
  const steps = exchangeSteps(exchange, isLast, threadIdle);
  const events = exchangeResponseEvents(exchange, isLast, threadIdle);
  const status = exchangeStatus(exchange, streamingBuffer, isLast, hasPriorActive, threadIsCC, threadIdle, threadAwaitingAnswer);
  const error = exchangeError(exchange);

  // Tell `scrollState` whether the agent is LIVE on this thread, which is the
  // one thing that decides whether the reader's scroll retires their standing
  // follow: fleeing a reply in flight does, browsing an idle thread does not.
  // Here because this is the component that already derives the status, and
  // `scrollState` deliberately cannot import `store` to derive it itself.
  //
  // Only the LAST exchange answers, since only it can be running. The cleanup
  // clears the answer rather than leaving it. A thread switch unmounts this
  // exchange, and the incoming thread's last exchange sets its own value.
  // Between the two nobody may read the thread they just left.
  const threadLive = exchangeMarksThreadLive(isLast, status, threadIdle);
  useEffect(() => {
    if (!isLast) return;
    setThreadLive(threadLive);
    return () => setThreadLive(false);
  }, [isLast, threadLive]);

  // Cap detection reads `ResponseGenerated.text` directly via
  // `exchangeEngineLimitDetail`. The cap is emitted with no preceding
  // TextStreamed, so it never lands in `responseTextRaw`, which only
  // concatenates streamed text. Without this side channel the agent appears to
  // stop silently mid-task.
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
  const dropsEarlierProse = hidesEarlierProse(events);
  const hasSteps = steps.length > 0 || events.some(e => e.type === 'step');

  // Is there a body to fold, and therefore a body to draw at all? One
  // definition, because `hasBody` on the panel below asks the same thing.
  //
  // Deliberately NOT `hasEvents`. A fold swaps the body for a `⋯` stub, so on
  // a turn whose body draws nothing it swaps nothing for a mark. In flight that
  // is a real case. A coding-agent turn emits a whitespace-only text event
  // before every tool call. A reader with steps off sees nothing else, so
  // `events.length` runs ahead of anything on screen.
  //
  // Asking `drawsResponseRow` instead leaves the control dead exactly while the
  // turn is blank, lighting up with its first drawn row. Turning steps OFF can
  // therefore unfold a step-only turn, which is right: it is showing nothing
  // either way.
  const canCollapse = hasResponse || events.some((e) => drawsResponseRow(e, showSteps));
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
      // openAppById, not a `apps.find(...)` on the cached list: it re-fetches
      // the registry on a miss before concluding the app is gone, matching the
      // trigger branch below. A suspended iOS PWA can miss the AppCreated SSE
      // frame, so the cache lags a freshly created app until this refetch.
      // `data-app-fragment` is the app fragment the link named, absent when it
      // named none, so a plain app link leaves an open app where it was.
      if (appId) void openAppById(appId, undefined, appTarget.dataset.appFragment);
      return;
    }

    const triggerTarget = (e.target as HTMLElement).closest('.trigger-link') as HTMLElement | null;
    if (triggerTarget) {
      e.preventDefault();
      const triggerId = triggerTarget.dataset.triggerId;
      // navigateToTrigger, not a `triggers.find(...)` on the cached list: it
      // re-fetches the registry on a miss before concluding the trigger is
      // gone, and names it in the toast if it really is. The app branch above
      // still reads its cached list, so a miss there is silent.
      if (triggerId) void navigateToTrigger(triggerId);
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
    // link the LLM writes. Without this the browser navigates to the relative
    // URL: an app entry lands on a file preview via the engine's /data/* static
    // mount, and a panel name on a 404 for a /data/<panel> folder.
    const anchorTarget = (e.target as HTMLElement).closest('a') as HTMLAnchorElement | null;
    if (anchorTarget) {
      const rawHref = anchorTarget.getAttribute('href') || '';
      const appTargetRef = extractAppTargetFromHref(rawHref);
      // Unconditional, like the .app-link branch above: openAppById re-fetches
      // on a cache miss. A recognized `app:` href must never fall through to
      // the terminal guard, which would blame the SCHEME for a stale cache.
      if (appTargetRef) {
        e.preventDefault();
        void openAppById(appTargetRef.appId, undefined, appTargetRef.fragment ?? undefined);
        return;
      }
      const triggerId = extractTriggerIdFromHref(rawHref);
      if (triggerId) {
        e.preventDefault();
        void navigateToTrigger(triggerId);
        return;
      }
      const navName = extractNavTargetFromHref(rawHref);
      if (navName) {
        e.preventDefault();
        handleNavigationRequest({ target: navName });
        return;
      }
      // A bare app-id/name href like `habit-tracker` — the LLM writes
      // `[Habit Tracker](habit-tracker)` by analogy to `[Notifications](notifications)`.
      // Not caught by extractAppTargetFromHref (no apps/ prefix, no app: scheme);
      // resolve it against the loaded apps list by id OR name. Runs AFTER nav so
      // reserved panel names still route to their panel. Without this the
      // browser navigates to the relative href, the SPA fallback serves the
      // shell, and the whole workspace reloads.
      const bareRef = extractBareAppRef(rawHref);
      if (bareRef) {
        const app = apps.find((a) => a.id === bareRef || a.name === bareRef);
        if (app) {
          e.preventDefault();
          openApp(app);
          return;
        }
      }
      // A path under the workspace's data/ tree, recognized by SHAPE. So it
      // works for a file the agent wrote seconds ago that the cached artifact
      // list has not caught up with. That is exactly when `linkifyPaths`
      // declines to rewrite, and this fallback is all that stands between the
      // click and a full workspace reload. Runs BEFORE the OS-open branch, so
      // an absolute /artifacts/… is never mistaken for a disk path.
      const dataPath = extractDataPathTarget(rawHref);
      if (dataPath) {
        e.preventDefault();
        openFilePreview(dataPath);
        return;
      }
      // A `file://` URL or an absolute filesystem path, such as a staged
      // release .dmg or an /Applications/… folder. Open it with the OS, never
      // via the in-app file preview or the /data/* static mount, which are for
      // workspace-relative paths only. Runs AFTER the app and nav extractors so
      // their absolute routes keep working. An http(s) URL returns null here
      // and keeps its browser or panel behavior.
      const localFile = extractLocalFileTarget(rawHref);
      if (localFile) {
        e.preventDefault();
        openLocalFile(localFile);
        return;
      }
      // TERMINAL GUARD: nothing above claimed this href, so it goes nowhere
      // useful. The branches above are a whitelist, and a whitelist is open at
      // the bottom. Closing it here makes the next unrecognized shape a toast
      // the user can read. See ADR 0038.
      //
      // Two failure modes, one guard. A href with NO scheme is a relative link
      // into the SPA, and there are no relative routes: the browser resolves it
      // against the workspace base, the SPA fallback answers with the app
      // shell, and the whole workspace reloads. A href whose scheme nothing can
      // open does nothing at all, and that is the worse of the two: the user
      // cannot tell it from a dead app. `trigger:` was this until it was
      // claimed, and the agent will invent another.
      //
      // Only a fragment (`#section`, which navigates nothing) and a scheme the
      // browser genuinely acts on pass through.
      //
      // An EMPTY href is still swallowed, since `[text]()` resolves to the
      // current URL and reloads exactly like any other unclaimed relative href.
      // It just can't be named in the message. Keyed so tapping the same dead
      // link twice replaces the toast instead of stacking a duplicate.
      if (!browserHandlesHref(rawHref) && !rawHref.startsWith('#')) {
        e.preventDefault();
        showToast(deadLinkMessage(rawHref), 'error', { key: DEAD_LINK_TOAST_KEY });
      }
    }
  }

  // Both turn controls change the height of every turn in the transcript. The
  // control the reader pressed is what holds still across it, via
  // `heldOnThePress`.
  //
  // Turning either ON also lifts THIS turn's fold, and only this turn's. A
  // folded turn draws no body. A reveal clicked from its header would land on
  // every other turn and do nothing where the click was made. The setting
  // stays transcript-wide; the unfold clears the local override hiding it here.
  // One-way, via `expandExchange` rather than a toggle: turning a reveal off
  // must never fold anything, since a fold is the reader's explicit act.
  //
  // Unconditional on the ON edge, and NOT gated on `canCollapse`. The turn this
  // fires on is often one where `canCollapse` is false BECAUSE the thing being
  // revealed is hidden. A folded step-only turn with steps off draws nothing,
  // so it reads as uncollapsible until the click that turns steps on. Gated,
  // that click leaves the key in the store and the turn folds back to `⋯` the
  // instant its steps become drawable. `expandExchange` no-ops when the key is
  // absent, so the unconditional call costs nothing on an unfolded turn.
  function reveal(setting: Signal<boolean>) {
    setting.value = !setting.value;
    if (setting.value) expandExchange(threadId, exchange.userSeq);
  }

  const toggleDetails = heldOnThePress(() => reveal(detailsExpanded));
  const toggleSteps = heldOnThePress(() => reveal(stepsExpanded));

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

  // Folding this turn. One definition, because the header's collapse control
  // and the `⋯` stub the fold leaves behind are two ways into the same action.
  // Anchored like the other two: a fold that shrinks the transcript past its
  // own pane clamps the offset, and the reader is owed their control back.
  const toggleCollapsed = heldOnThePress(() => toggleExchangeCollapsed(threadId, exchange.userSeq));
  const toggleInitiator = heldOnThePress(() => toggleInitiatorCollapsed(threadId, exchange.userSeq));

  // The header's three controls, rendered in every state (see `turnControls`):
  // the collapse control is one of them, so a collapsed turn needs the group
  // more than any other, not less.
  const turnControlsSlot = turnControls({
    detailsOn: showDetails,
    stepsOn: showSteps,
    collapsed: isCollapsed,
    collapsible: canCollapse,
    onToggleDetails: toggleDetails,
    onToggleSteps: toggleSteps,
    onToggleCollapsed: toggleCollapsed,
  });

  const { visibleEvents, collapsedFallbackText } = useMemo(() => {
    let visible: ResponseEvent[] = [];
    let fallback = '';
    if (hasEvents) {
      if (showDetails || !dropsEarlierProse) {
        // The render window's head clamp, applied HERE and nowhere earlier.
        // Every verdict above reads the full `events`: whether this turn has
        // sections, whether it is an empty continuation, whether its divider
        // body is suppressed. Those describe the TURN and must not change
        // because its head is off screen.
        visible = rowsHidden > 0 ? events.slice(rowsHidden) : events;
      } else {
        // A collapsed turn is already down to a handful of rows. The clamp has
        // nothing to save there, and would cut into what the collapse chose to
        // keep. `rowsHidden` counts the UNCOLLAPSED list either way.
        const collapsed = getCollapsedVisibleEvents(events);
        visible = collapsed.visibleEvents;
        if (collapsed.needsFallback) {
          fallback = responseHtmlCombined;
        }
      }
    }
    return { visibleEvents: visible, collapsedFallbackText: fallback };
  }, [hasEvents, showDetails, dropsEarlierProse, events, responseHtmlCombined, rowsHidden]);

  // Sections tagged with each section's base index in `visibleEvents`, so
  // `renderResponseEvents` can key rows stably as the list grows during
  // streaming. `splitEventSections` drops the break markers, so re-walking
  // `visibleEvents` recovers each section's offset.
  //
  // The open cost is bounded twice, both by ThreadView: which EXCHANGES render,
  // on a step budget, and how many of the FLOOR exchange's rows do, on a row
  // budget (`threadWindow.ts`). `visibleEvents` above already carries the
  // second, so this walks only what is drawn.
  //
  // The clamp that used to sit here was a different thing: it shipped a "Show
  // earlier steps" expander the reader found confusing, and THAT is not coming
  // back. The head arrives by scrolling now, with no control of its own.
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

  // Exactly one running-text shimmer at a time, and it has to be one the reader
  // can SEE. While the live step row is on screen its shimmer is the affordance,
  // so the "Working" label drops to a plain static one. Otherwise the label
  // shimmers as the sole affordance.
  //
  // Two halves, because drawn and seen are different questions. `liveStepIndex`
  // answers the first from data alone: steps hidden, this exchange collapsed, or
  // no pending step. The hook answers the second, and it is the half that used
  // to be missing. A coding-agent turn always carries a live row, derived by
  // `needsLiveThinkingRow` for every gap between calls. So a tall turn read
  // "Working" over finished checks, with its only shimmer far below the fold.
  //
  // No row element means no shimmer to defer to, so the label takes it. That is
  // the safe direction: the failure this fixes is a label that stayed plain.

  // `useState`, so the setter IS the ref callback: a stable identity, which is
  // what stops Preact re-running the ref on every render of a live turn.
  const [liveStepRow, setLiveStepRow] = useState<HTMLElement | null>(null);
  const liveRowIndex = liveStepIndex(showSteps, isCollapsed, visibleEvents);
  // Gated on the index because the ref does NOT clear itself when a row stops
  // being the live one. Preact clears a ref only on unmount, or when a
  // DIFFERENT ref replaces it on the same element. So a row that settles IN
  // PLACE leaves the state pointing at itself: held on a permission card, or
  // killed with the turn. Gating here is what tears the observer down.
  const markedRow = liveRowIndex >= 0 ? liveStepRow : null;
  const rowOnScreen = useOnScreenInTranscript(markedRow);
  const liveStepOnScreen = markedRow !== null && rowOnScreen;

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
    // The streaming buffer's html changes every token, so its linkify opts out
    // of the LRU cache, mirroring `renderMarkdown`'s `cache: false` above.
    // Per-token html would otherwise thrash the cache and evict stable entries.
    () => linkifyPaths(responseHtmlCombined, artifactPaths, apps, { cache: !streamingBuffer }),
    [responseHtmlCombined, artifactPaths, apps, streamingBuffer],
  );

  const responseTerminated = isTerminated(status) || exchange.questionOvertaken === true;

  const initiator = useMemo(
    () => describeInitiator(exchange, userMessageHtml, userImageHashes, threadId, responseTerminated, threadIsCC, threadCodingAgent, proposedChangeDesc, proposedChangeFileCount, { eventType: matchedEventType, eventId: matchedEventId, payloadJson: matchedPayloadJson }),
    [exchange, userMessageHtml, userImageHashes, threadId, responseTerminated, threadIsCC, threadCodingAgent, proposedChangeDesc, proposedChangeFileCount, matchedEventType, matchedEventId, matchedPayloadJson],
  );
  const isChangePanel = isChangeLifecycleEvent(exchange.userEvent);
  // Card-less treatment. A human chat message renders as a right-aligned
  // accent-tinted bubble and change-lifecycle turns render flat. Both drop the
  // actor chip, moving attribution to the clickable timestamp or summary. The
  // predicate is event-type based, NOT label-based. A user-driven control turn
  // keeps the chip slot, rendered iconless with the action AS the label (see
  // `actionInitiator`). Question dividers keep their agent chip.
  const isUserMessageBubble = isUserBubbleEvent(exchange.userEvent) && initiator.variant === 'user';
  // Both of those are exempt from the fold, on report. A change turn's body is
  // a summary, a description and a file list; a user message is the reader's
  // own text. The control cost a row of chrome to fold a few short lines.
  const canCollapseInitiator = !isChangePanel && !isUserMessageBubble
    && (!!initiator.summary || !!initiator.details);
  const isInitiatorCollapsed = canCollapseInitiator
    && collapsedInitiators.value.has(`${threadId}:${exchange.userSeq}`);
  const changeId = exchangeChangeId(exchange, isChangePanel, threadIsCC);
  // A queued follow-up shows a "Queued" tag in its own bubble header, where
  // dividers show "Answered ✓". A faux "Lucidos Agent" response panel below it
  // would misattribute it: the message is the user's, and a stack of them
  // should each read as waiting.
  const isQueuedUserMessage = !!isQueued && isUserMessageBubble;
  const queuedMessageId = isQueuedUserMessage ? exchange.userEvent._eventId : undefined;
  // The caller is mid-sentence, so nothing is in flight behind this bubble and
  // no panel belongs under it. It draws the bubble and stops there.
  const isLiveUtterance = isLiveUtteranceRow(exchange.userEvent);
  // The trash button lives INSIDE the status label, an existing `display: flex`
  // row, rather than in a separate wrapper. "Queued" and the trash then stay on
  // one line using only CSS that already ships.
  const queuedStatus = (
    <span class="exchange-status-label exchange-status-queued">
      {'Queued'}
      {queuedMessageId && (
        <button
          type="button"
          class="icon-btn inline-icon queued-message-remove"
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
  const isUnansweredDivider = dividerBodyIsSuppressed(exchange, events);
  // Change lifecycle, abort-boundary, cancel-boundary and answer-less question
  // dividers are terminal. They have no response, just the initiator panel with
  // optional actions. The exception is a change banner whose session KEPT
  // WORKING after the apply, folding the continuation into this exchange as
  // steps. It must render its body, or that work and its follow-up proposal are
  // invisible between two "Change applied" rows.
  const isChangeContinuation = isChangePanel && changePanelHasContinuation(exchange);
  // Same exception, other boundary: an abort or cancel boundary that ACQUIRED
  // work must render it. The boundary is a statement about the turn that ended,
  // not a promise that nothing follows, and something can legitimately land
  // under it. The sharpest case is an event-wait delivery, whose anchor is not
  // an exchange-start type, so its whole turn folds in here as steps.
  //
  // Suppressing that hides a turn which applied a change, spawned a sub-thread
  // and wrote a full summary, behind a bare "Response interrupted". A stepless
  // boundary still renders bare, the common case the panel was written for.
  //
  // The test is RENDERABLE content, not `hasEvents`. A boundary picks up the
  // drain of whatever the teardown killed, and a coding-agent subprocess signs
  // off with a bare `"\n\n"`. That becomes a `text` event counting toward
  // `hasEvents` and drawing nothing. The switch-teardown boundary then gets a
  // response panel whose only content is a "Working" badge over a stopped
  // engine.
  const isTerminatedContinuation = (isAbortPanel || isCancelPanel)
    && (hasResponse || hasRenderableResponseContent(events));
  // The user's Stop-waiting turn. Nothing resumes out of it, since a stop is
  // the one resolution that re-enters nothing. So unlike the abort and cancel
  // boundaries it takes no continuation exception: the header line IS the whole
  // turn, and a response panel would be a status badge over an empty body.
  const isEventWaitStopPanel = isUserStoppedWait(exchange.userEvent);
  const showResponsePanel = (!isChangePanel || isChangeContinuation) && (!isAbortPanel || isTerminatedContinuation) && (!isCancelPanel || isTerminatedContinuation) && !isEventWaitStopPanel && !isUnansweredDivider && !isEmptyContinued && !isQueuedUserMessage && !isLiveUtterance && (hasResponse || hasEvents || showStatus);
  let initiatorActions: ComponentChildren | undefined;
  if (isChangePanel) {
    initiatorActions = changeActions(
      (exchange.userEvent as { change_id?: string }).change_id,
      exchange.userEvent.type === 'ChangeApplyFailed',
      // ChangeApplied always resolves to at least a Revert button. Reserving
      // the footer row while the Change row loads stops the buttons shifting
      // the panel down on first open, mirroring the body's ChangeProposed seed.
      exchange.userEvent.type === 'ChangeApplied',
    );
  } else if (isAbortPanel && isContinuableAbort) {
    initiatorActions = <ContinueButton threadId={threadId} />;
  }
  const executor = describeExecutor(threadIsCC, threadCodingAgent);

  function renderResponseEvents(eventsList: ResponseEvent[], baseIndex = 0) {
    return eventsList.map((evt, i) => {
      // Key by ABSOLUTE index in `visibleEvents`, not the local per-section
      // slice index. `splitEventSections` hands each section its base offset,
      // so a row's key is stable even as earlier sections grow during
      // streaming. That keeps the visible rows stable rather than re-rendering
      // the whole tail on each streamed event.
      const k = baseIndex + i;
      if (evt.type === 'text' && evt.md?.trim()) {
        // Classed so the chunk can own the space around itself. Interleaved
        // with step rows, a markdown paragraph's bottom-only margin is all that
        // separates prose from a log row, putting the air on one side. See
        // `.response-chunk` in chat/response.css.
        return <div key={`t${k}`} class="response-chunk" dangerouslySetInnerHTML={{ __html: visibleTextHtmls.get(evt)! }} />;
      }
      // `evt.type === 'step'` is `isStepMechanics` spelled inline, for the type
      // narrowing `InlineStep` needs. It is the ONLY row the toggle hides.
      //
      // The live row is marked so the header label can read where it sits.
      // MOVING the mark is safe in either direction. Preact clears the old ref
      // during the diff and applies the new one after it. So a clear can never
      // land on top of a set. Losing the mark entirely is the case Preact does
      // not handle, and the reader above gates on the index for exactly that.
      if (evt.type === 'step' && showSteps) return <InlineStep key={`s${k}`} event={evt} rowRef={k === liveRowIndex ? setLiveStepRow : undefined} />;
      if (evt.type === 'image') return <GeneratedImage key={`img${k}`} event={evt} />;
      if (evt.type === 'checkpoint') return <CheckpointCard key={`cp${k}`} event={evt} />;
      // Ungated, like the event row below. It is what the caller HEARD, no
      // audio is kept, and the written answer beside it is a different thing:
      // the talker says what an answer means rather than reading it out.
      if (evt.type === 'spoken_reply') return <SpokenReply key={`sr${k}`} event={evt} />;
      // Ungated, like every other marker. The park is the transcript's only
      // record that the thread subscribed to something. The clock indicator
      // holds the LIVE half and drops the wait as it resolves. A toggle
      // defaulting to off would leave a resolved wait recorded nowhere.
      if (evt.type === 'event_wait') return <EventWaitRow key={`ew${k}`} event={evt} />;
      if (evt.type === 'empty') return <div key={`e${k}`} class="response-empty-note">{'The model returned an empty response.'}</div>;
      return null;
    });
  }

  // Identity for keyboard turn-nav's Enter toggle (see
  // scrollState.toggleNavigatedTurnCollapsed): which collapse store the ⌘↑/⌘↓-
  // highlighted turn folds. Response body wins; a response-less divider/change turn
  // falls back to its initiator panel; absent when neither is collapsible so Enter
  // is a no-op there.
  const collapseKind = canCollapse ? 'response' : canCollapseInitiator ? 'initiator' : undefined;

  return (
    <div class="chat-exchange" data-event-id={exchange.userEvent._eventId} data-change-id={changeId || undefined}
         data-thread-id={threadId} data-user-seq={exchange.userSeq} data-collapse-kind={collapseKind}>
      <InitiatorPanel
        initiator={isQueuedUserMessage
          // Appended, not replaced. A spoken message already put its own chip
          // in this slot, and both facts are true of a queued utterance.
          ? { ...initiator, status: <>{initiator.status}{queuedStatus}</> }
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
        onToggle={canCollapseInitiator ? toggleInitiator : undefined}
      />

      {showResponsePanel && (
        <ResponsePanel
          executor={executor}
          onExecutorClick={(e) => openInfoPanel('executor', e)}
          controls={turnControlsSlot}
          hasBody={canCollapse}
          status={showStatus && shouldShowResponseStatusBadge(exchange.userEvent, statusClass) ? (
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
          collapsed={isCollapsed}
          onToggle={canCollapse ? toggleCollapsed : undefined}
        >
          {hasEvents && hasSections ? (
            renderedSections.map(({ events: section, base }) => (
              <div class="response-content markdown-content" key={`sec-${base}`} onClick={handleLinkClick}>
                {renderResponseEvents(section, base)}
              </div>
            ))
          ) : (
            <div class="response-content markdown-content" onClick={handleLinkClick}>
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
        // The failure card is addressed by the `ResponseFailed`'s OWN event id,
        // not the exchange's. A notification raised by a failure deep-links to
        // that event. `ResponseFailed` folds into the owning exchange as a
        // terminal step, so the root's `data-event-id` is the turn's STARTER
        // and never matches. Stamping the card lets `scrollToEventAndPulse`
        // land on the failure itself and `isEventInViewport` report it.
        //
        // It is the only step-level surface needing this: every other
        // deep-linkable event either starts its own exchange or is addressed by
        // `data-change-id`. Inline steps are NOT stamped, since the "Show
        // steps" toggle can hide them and an id there resolves only sometimes.
        <div class="exchange-error" data-event-id={error.eventId || undefined}>
          <strong>Event stream error</strong>
          <p>{error.message}</p>
        </div>
      )}
    </div>
  );
}

/** Are two gated-step mark sets the same? Absent and empty are one state: a
 *  resolution deletes the last entry rather than dropping the set, so the two
 *  spellings of "nothing is marked" must not read as a change.
 *
 *  Iterated rather than compared by identity, because a FULL rebuild allocates
 *  fresh sets. See the `blockedStepSeqs` line in `chatExchangePropsEqual`. */
function sameStepSeqs(a: Set<number> | undefined, b: Set<number> | undefined): boolean {
  if (a === b) return true;
  if (!a || !b) return (a?.size ?? 0) === (b?.size ?? 0);
  if (a.size !== b.size) return false;
  for (const seq of a) if (!b.has(seq)) return false;
  return true;
}

/** Custom prop equality for the `memo`-wrapped `ChatExchange` below.
 *
 *  Default `memo` shallow-compares props, and a from-scratch `computeExchanges`
 *  pass produces fresh Exchange objects, so it would re-render every child on
 *  every SSE event. A **content-relevant** fingerprint is compared instead:
 *
 *   - `revision`, the in-place mutation counter, captured as a primitive at
 *     render time. The incremental grouping cache keeps Exchange objects
 *     identity-stable and mutates them in place, so when
 *     `prev.exchange === next.exchange` every field compare below is a
 *     self-comparison. The captured revisions are the only honest signal.
 *   - `userSeq`, the exchange boundary.
 *   - `steps.length` plus the last step's `seq`: a new event landed here.
 *   - `questionOvertaken`, flipped when the agent ignored a question.
 *   - `continuationMoved`, the turn handed to a later exchange, which
 *     finalizes this one's pending Thinking marker.
 *   - `blockedStepSeqs` / `deniedStepSeqs`, a permission decision on a call
 *     this exchange owns. They are the one mark written from OUTSIDE, by a
 *     card that is its own later exchange, so nothing else here moves with it.
 *
 *  All other props are primitives or strings, compared with Object.is. */
export function chatExchangePropsEqual(prev: Props, next: Props): boolean {
  if (prev.revision !== next.revision) return false;
  if (prev.streamingBuffer !== next.streamingBuffer) return false;
  if (prev.isLast !== next.isLast) return false;
  if (prev.isQueued !== next.isQueued) return false;
  if (prev.threadId !== next.threadId) return false;
  if (prev.hasPriorActive !== next.hasPriorActive) return false;
  if (prev.priorModel !== next.priorModel) return false;
  if (prev.priorEffort !== next.priorEffort) return false;
  if (prev.isContinuableAbort !== next.isContinuableAbort) return false;
  if (prev.threadIsCC !== next.threadIsCC) return false;
  if (prev.threadCodingAgent !== next.threadCodingAgent) return false;
  if (prev.threadIdle !== next.threadIdle) return false;
  if (prev.threadAwaitingAnswer !== next.threadAwaitingAnswer) return false;
  if (prev.threadCanceling !== next.threadCanceling) return false;
  // Without this the memo swallows every scroll-up round into the floor turn:
  // the window grows, nothing else about the turn changes, and the head the
  // reader scrolled up for never draws.
  if (prev.rowsHidden !== next.rowsHidden) return false;
  if (prev.proposedChangeDesc !== next.proposedChangeDesc) return false;
  if (prev.proposedChangeFileCount !== next.proposedChangeFileCount) return false;
  if (prev.matchedEventType !== next.matchedEventType) return false;
  if (prev.matchedEventId !== next.matchedEventId) return false;
  if (prev.matchedPayloadJson !== next.matchedPayloadJson) return false;
  const a = prev.exchange;
  const b = next.exchange;
  if (a.userSeq !== b.userSeq) return false;
  if (a.questionOvertaken !== b.questionOvertaken) return false;
  if (a.continuationMoved !== b.continuationMoved) return false;
  if (a.steps.length !== b.steps.length) return false;
  const aLast = a.steps[a.steps.length - 1]?.seq;
  const bLast = b.steps[b.steps.length - 1]?.seq;
  if (aLast !== bLast) return false;
  // A permission card marks a call in the PREVIOUS exchange, whose own steps do
  // not change, so every field above is identical across that mark. The
  // incremental fold bumps `revision` for it. A FULL rebuild allocates fresh
  // objects carrying no revision, and there the fingerprint is the only thing
  // deciding. Without these two the held call keeps rendering "In progress"
  // after an out-of-order event forced the rebuild.
  if (!sameStepSeqs(a.blockedStepSeqs, b.blockedStepSeqs)) return false;
  if (!sameStepSeqs(a.deniedStepSeqs, b.deniedStepSeqs)) return false;
  return true;
}

/** Memo-wrapped public component. Drops the 28 unchanged sibling re-renders
 *  on every per-SSE-event ThreadView re-render of the heavy thread. */
export const ChatExchange = memo(ChatExchangeImpl, chatExchangePropsEqual);

/** Whether the response panel gets a status badge at all. Two turns state
 *  their own outcome in the panel ABOVE the response, so a second rendering is
 *  noise at best and a contradiction at worst:
 *
 *  - A question card whose own Cancel-as-picked button carries the "Canceled ✕"
 *    signal.
 *  - A **switch-teardown boundary**. Its initiator panel reads "Paused by
 *    restart", the engine promising to bring the turn back. The badge under it
 *    would be the "Aborted ⚠" the drain earns from the stale detector.
 *    Painting a failure affordance on a switch is what
 *    `docs/plans/2026-08-06-paused-only-for-a-user-initiated-switch.md` removed
 *    from the status dot. Narrowed to the switch fingerprint on purpose: an
 *    ordinary abort boundary CAN acquire a live turn, and that turn needs its
 *    "Working" badge. */
export function shouldShowResponseStatusBadge(
  userEvent: ThreadEvent,
  statusClass: string,
): boolean {
  if (userEvent.type === 'UserQuestionAsked' && statusClass === 'canceled') return false;
  return !abortPromisesAutoResume(userEvent);
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
    // No summary line: the event row in the body says "Trigger fired: <name>",
    // so a header saying "Trigger fired" above it states the same thing twice.
    // Same as `ChildThreadCompleted` below, whose row has owned its own prefix
    // all along. The route popover is unaffected either way: it builds its own
    // Trigger row in `renderTriggerOrigin` and never reads this.
    case 'TriggerStarted':           return '';
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
    case 'EventWaitCanceled':        return eventWaitStoppedSummary(ev.reason);
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

/** Map a `MessageOrigin` to its display icon and label. The chip answers "who
 *  decided this" from a closed set of actors:
 *
 *  - **You**, a real browser device.
 *  - **Lucidos Agent**, the LLM acting for the user, as the Lucidos mark.
 *  - **Lucidos Engine**, deterministic engine work, as the SAME mark. The label
 *    is what tells it apart from the agent.
 *  - **System**, the host killing the process.
 *  - **API caller**, an external HTTP caller that did not self-identify.
 *
 *  "You" is reserved for `kind: device`, a browser session bound to a known
 *  device row. Any other human-mode origin renders as "API caller", so an
 *  unauthenticated POST can never impersonate the user in the timeline. The
 *  popover still discloses the origin kind, user-agent and workspace name.
 *
 *  Lives in the view layer rather than the store, because EVERY actor icon is a
 *  component, matching how `describeExecutor` resolves the same glyphs. The
 *  store owns the LABELS and nothing else, staying free of UI components.
 *  `ApiPlugIcon` records why the API caller gets a plug; the System chip's own
 *  reason sits at its branch below. */
export function actorInitiator(actor: MessageOrigin | undefined): { icon: ComponentChildren; label: string } {
  // The host system killed the process (engine shutdown, OS signal, crash), so
  // the power symbol rather than the Lucidos mark the deliberate engine wears.
  if (actor?.kind === 'system') return { icon: <PowerIcon />, label: SYSTEM_LABEL };
  if (actor?.kind === 'device') return { icon: <PersonIcon />, label: 'You' };
  switch (originMode(actor)) {
    case 'human':  return { icon: <ApiPlugIcon />, label: API_CALLER_LABEL };
    case 'agent':  return { icon: <LucidosGlyph />, label: LUCIDOS_AGENT_LABEL };
    case 'engine': return { icon: <LucidosGlyph />, label: ENGINE_LABEL };
  }
}

/** Build a `'user'`-variant initiator descriptor with the standard human chip
 *  (icon + "You" label) and a caller-supplied summary/details/accent. Shared by
 *  every arm where the device-owner is the initiator (MessageReceived from a
 *  device, divider-starter ActionRequired events, …). */
function youInitiator(rest: Partial<InitiatorDescriptor> = {}): InitiatorDescriptor {
  return { variant: 'user', icon: <PersonIcon />, label: 'You', ...rest };
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
 *  resolved: `'canceled'` only when the USER explicitly dismissed it,
 *  `'superseded'` when their follow-up replaced the question, and `'dropped'`
 *  for every other turn-ended-without-a-response cause (system abort, error, the
 *  agent racing past the prompt). */
type DividerTerminalKind = 'canceled' | 'superseded' | 'dropped';

/** Resolution status for question and permission dividers, shown in the
 *  initiator header. The header describes what happened to the PROMPT, never
 *  the turn:
 *
 *  - "Answered" or "Resolved" (✓) when the user responded.
 *  - "Canceled" (✕) when they dismissed it.
 *  - "Unanswered" or "Unresolved" when the turn ended for any other reason.
 *  - "Needs your answer" while pending.
 *
 *  The response panel and the abort boundary carry the turn's own terminal
 *  cause. The header must NOT impersonate it: a system abort rendering here
 *  as the user-driven "Canceled" contradicts the "Aborted" panel below it.
 *  Reuses the response panel's `.exchange-status-*` classes, so the glyphs
 *  match. */
function dividerStatus(
  resolved: boolean,
  resolvedLabel: string,
  droppedLabel: string,
  terminal: DividerTerminalKind | null,
): ComponentChildren {
  if (resolved) return <span class="exchange-status-label exchange-status-done">{resolvedLabel}<span class="exchange-status-check">{'✓'}</span></span>;
  if (terminal === 'canceled') return <span class="exchange-status-label exchange-status-canceled">{'Canceled'}<span class="exchange-status-x">{'✕'}</span></span>;
  // Neutral, like a Codex follow-up redirect: the user steered, they did not
  // dismiss. A "Canceled ✕" here would blame them for a question they replied
  // past.
  if (terminal === 'superseded') return <span class="exchange-status-label exchange-status-dropped">{'Superseded'}</span>;
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
  /** First line of the in-thread `ChangeProposed` description and its file
   *  count, forwarded to <ChangeBody> for the change-lifecycle arms. The body
   *  then paints at full height on first open. */
  proposedChangeDesc?: string,
  proposedChangeFileCount?: number,
  /** The event a *thread subscription* delivered, already resolved through
   *  this exchange's `UserPromptInjected.delivered_event_id` (see
   *  `buildDeliveredEventInfo`). Undefined for every exchange that is not such
   *  a delivery.
   *
   *  ONE object rather than three trailing `string | undefined` params. At the
   *  end of a twelve-argument positional list, same-typed neighbours mis-order
   *  with no type error, and inserting one silently re-binds a caller's
   *  argument. The fields are still flat primitives, never the payload object,
   *  so `chatExchangePropsEqual` compares them without a deep walk. */
  matched?: { eventType?: string; eventId?: string; payloadJson?: string },
): InitiatorDescriptor {
  const ev = exchange.userEvent;
  // Ahead of the switch, because the row wears a `MessageReceived` and would
  // otherwise take that arm and draw an empty bubble. The caller is speaking
  // and no words exist yet, so the bubble holds a pulse where they will go.
  if (isLiveUtteranceRow(ev)) {
    return youInitiator({ details: <LiveUtteranceBody />, status: <SpokenChip /> });
  }
  const summary = initiatorSummary(ev);
  switch (ev.type) {
    case 'TriggerStarted':
      // The row carries the subject now ("Trigger fired: <name>"), so the panel
      // header drops its own summary line rather than saying it twice.
      return {
        variant: 'trigger',
        icon: <TriggerFiredIcon />,
        label: ENGINE_LABEL,
        details: <TriggerFiredBody event={ev} />,
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
      // engine aborts keep the Lucidos mark and system ones the power symbol.
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
    case 'EventWaitCanceled':
      // Only a user stop reaches here: every other cause stays a step inside
      // the turn it happened in (see `isExchangeStartEvent`). Same iconless
      // boundary style as the cancel above, and for the same reason: the action
      // IS the header, and the chip opens the popover that names the device
      // that pressed Stop waiting (read off this event's own `actor`).
      return actionInitiator(summary);
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
        // An event delivery, resolved through `delivered_event_id`, is the
        // one injection whose text is NOT its content: the prose is the model's
        // prompt and carries the payload as raw JSON. Name the event instead
        // and fold the payload away. Falls back to the prose whenever the link
        // is absent (every other injection, legacy rows) or unresolved (the
        // delivery scrolled out of the loaded window).
        //
        // No summary line on the delivery: its event row already reads "Event
        // arrived: <event>", so a header saying the same words prints it twice.
        // Same as the trigger and the child callback, whose rows own their
        // prefixes too.
        summary: matched?.eventType ? undefined : summary,
        details: matched?.eventType
          ? <EventDeliveryBody eventType={matched.eventType} eventId={matched.eventId} payloadJson={matched.payloadJson} />
          : <MarkdownBlock html={userMessageHtml} />,
      };
    case 'MessageReceived': {
      const details = userMessageHtml || userImageHashes.length > 0
        ? <UserMessageBody html={userMessageHtml} imageHashes={userImageHashes} />
        : undefined;
      if (ev.origin?.kind === 'api' || modeToInitiator(ev.mode) === 'system') {
        return { variant: actorVariant(ev.origin), summary, details, ...actorInitiator(ev.origin) };
      }
      // A spoken message says so. The composer stays live during a call (ADR
      // 0148), so the transcript interleaves speech and typing and the reader
      // otherwise cannot tell which they did. The mark carries that one fact,
      // and the bubble under it is the one a typed message gets.
      if (ev.voice_session_id) {
        return youInitiator({ details, status: <SpokenChip /> });
      }
      return youInitiator({ details });
    }
    // A call greeting, said before anything had started a turn, so it opened a
    // boundary of its own (`exchange-grouping`). Every other spoken reply is a
    // step and renders through `exchangeResponseEvents` instead.
    case 'SpokenReplyGenerated':
      return {
        variant: 'lucidos',
        icon: <LucidosGlyph />,
        label: LUCIDOS_AGENT_LABEL,
        details: (
          <SpokenReply
            event={{ type: 'spoken_reply', text: ev.text, interrupted: ev.interrupted === true }}
          />
        ),
      };
    case 'SpokenMessageReceived':
      // The caller's own words, as the ordinary user bubble. One act, one
      // shape: this is the same utterance a delegated one is, and which model
      // fielded it is not the reader's distinction. `userMessageHtml` carries
      // the words, so both arms render through one path.
      //
      // A wordless utterance draws no bubble, the same guard the arm above
      // takes. Nothing carries images: the caller is speaking.
      return youInitiator({
        details: userMessageHtml
          ? <UserMessageBody html={userMessageHtml} imageHashes={[]} />
          : undefined,
        status: <SpokenChip />,
      });
    case 'ChildThreadCompleted':
      // The EventBus fan-in path raises this on the parent when a child thread
      // reaches a terminal event. That is deterministic engine plumbing, not
      // LLM work, so attribute it to the engine like every other
      // engine-injected event. The child agent's authored summary lives in the
      // card body. The chip is non-clickable, since the title-link is the
      // origin affordance.
      return {
        variant: 'system',
        icon: <LucidosGlyph />,
        label: ENGINE_LABEL,
        actorClickable: false,
        details: (
          <ChildCompletionRow
            childThreadId={ev.child_thread_id}
            childThreadTitle={ev.child_thread_title}
            status={ev.status}
            summary={ev.summary}
            pendingChangeIds={ev.pending_change_ids}
          />
        ),
      };
    case 'UserQuestionAsked': {
      // The agent ASKS the question; attribute the divider to it (Lucidos Agent
      // or Claude Code), with a resolution status in the header. Resolution
      // lives on this exchange's steps as UserQuestionAnswered; matched by
      // tool_use_id so a stale Answered from a different question can't bleed in.
      const answered = findQuestionAnswer(exchange, ev.tool_use_id);
      // A question resolved WITHOUT an answer still carries a
      // UserQuestionAnswered, which findQuestionAnswer returns. Exclude those
      // from "Answered" and let each carry its own status instead.
      const unanswered = questionDividerResolution(exchange);
      const agent = describeExecutor(threadIsCC, threadCodingAgent);
      return {
        variant: 'lucidos',
        icon: agent.icon,
        label: agent.label,
        status: dividerStatus(
          !!answered && !unanswered,
          'Answered',
          'Unanswered',
          unanswered ?? (responseTerminated ? 'dropped' : null),
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
