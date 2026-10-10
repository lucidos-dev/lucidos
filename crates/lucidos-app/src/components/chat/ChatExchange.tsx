import { Fragment, type ComponentChildren } from 'preact';
import { WidgetCard } from '../widgets/WidgetCard';
import { ReplyHtml } from '../widgets/useWidgetEmbedSlots';
import type { ReadonlySignal, Signal } from '@preact/signals';
import { memo } from 'preact/compat';
import { useMemo, useState } from 'preact/hooks';
import { loadedOr } from '../../store/types';
import type { ResponseEvent, App } from '../../store/types';
import type { CodingAgent } from '../../api/types';
import type { Exchange, ReadMarker, StoredEvent, ThreadEvent, MessageOrigin, ResolvedPermission, TypedAnswer } from '../../store/thread-events';
import { ENGINE_LABEL, SYSTEM_LABEL, API_CALLER_LABEL, LUCIDOS_AGENT_LABEL, abortPromisesAutoResume, exchangeUserMessage, isUserBubbleEvent, exchangeUserImageHashes, exchangeTimestamp, exchangeResponseTimestamp, messageReadTimestamp, exchangeResponseText, exchangeEngineLimitDetail, exchangeSteps, exchangeResponseEvents, exchangeStatus, exchangeError, exchangeStarterId, dividerBodyIsSuppressed, hasRenderableResponseContent, isEmptyContinuedExchange, questionDividerResolution, changePanelHasContinuation, findCommandPermissionResolution, findMcpPermissionResolution, findPermissionResolution, findQuestionAnswer, isChangeLifecycleEvent, isLivePartialRow, isLiveReplyRow, isLiveUtteranceRow, isSpeechOnlyTurn, turnBodyFolded, modeToInitiator, originMode, continuationStartedSummary, responseAbortedSummary, eventWaitStoppedSummary, isTurnlessBoundary, isUnsentExchange, agentMessageSender, waitReentryReason, RESPONSE_CANCELED_SUMMARY } from '../../store/thread-events';
import { LucidosGlyph } from '../shared/LucidosMark';
import { artifacts, appsList, stepsExpanded, detailsExpanded, collapsedExchanges, toggleExchangeCollapsed, expandExchange, collapsedInitiators, toggleInitiatorCollapsed, toggleMessageRoutePanel } from '../../store/store';
import { discardUnsentMessage, editQueuedMessage, removeQueuedMessage, retryUnsentMessage } from '../../store/actions/chat';
import { unsentMessages } from '../../store/unsentMessages';
import { withScrollAnchor } from './CreateThreadView';
import { QuestionBody } from './QuestionCard';
import { originPopoverHasContent } from './messageOrigin';
import { pendingAnswers, unsentPicks } from '../../store/pendingDecisions';
import { CommandPermissionBody, McpPermissionBody, PermissionBody, engineResolutionNote, pendingVerdicts } from './PermissionCard';
import { ChildCompletionRow, ChildMovedOutRow, ChildStoppedRow } from './ChildCompletionRow';
import { drawsResponseRow, liveStepInBody, responseBody, type BodyRow } from '../../store/event-rendering';
import { Disclosure } from '../shared/Disclosure';
import { HELD_CALLBACK_NOTE, statusLabel as getStatusLabel, isActive as isStatusActive, isTerminated } from '../../store/exchange-status';
import type { ExchangeStatus } from '../../store/exchange-status';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { linkifyPaths } from '../../utils/linkifyPaths';
import { handleMarkdownLinkClick } from '../shared/markdownLinkClick';
import { FormRequestRow } from './FormRequestRow';
import { NO_SIDE_QUESTIONS, SideQuestionGroup, placeInBody } from './SideQuestionCard';
import type { SideQuestion } from '../../store/sideQuestions';
import { ChangeEventRow, CheckpointCard, ContinueButton, EventDeliveryBody, EventWaitRow, FileList, HeldMessageRow, GeneratedImage, InitiatorPanel, InlineStep, LivePartialBody, SubAgentStepGroup, LiveUtteranceBody, MarkdownBlock, ResponsePanel, ResumeCard, SpokenChip, SpokenReply, TriggerFiredBody, UserMessageBody, changeAccent, describeExecutor, boundaryCard, turnControls } from './chat-exchange-parts';
import type { BoundaryTurn } from './chat-exchange-parts';
import { EditIcon, TrashIcon, PowerIcon, PersonIcon, ApiPlugIcon, TriggerFiredIcon, WarningIcon, ContinuedIcon } from '../shared/icons';
import { useOnScreenInTranscript } from '../../hooks/useOnScreenInTranscript';
import { engineReasonHeadline } from '../../utils/engineEventExplainers';

// Stable refs so the `loadedOr` fallback does not yield a fresh [] each render.
// Without these, every dependent useMemo invalidates on every render while
// artifacts or apps are not loaded.
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
  /** Whether the agent has read the message that opened this exchange.
   *  Computed in `renderExchanges` by `readMarkers`, which needs every
   *  exchange to know where a coding agent's markers start. */
  readMarker?: ReadMarker;
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
   *  card, so a just-answered divider keeps reading "Working" during the
   *  answer-to-resume gap. */
  threadAwaitingAnswer: boolean;
  /** Folds after the card awaiting an answer, so the agent has not read it
   *  (`exchangeStatus`). */
  behindOpenQuestion?: boolean;
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
  /** The in-thread `ChangeProposed` description, its file count, and the
   *  `ChangeSummarized` summary of that description, for this exchange's
   *  change_id (built once per thread in `renderExchanges`). Seeds
   *  <ChangeEventRow> so a change-lifecycle panel paints at its final height on
   *  first open, before the per-id `Change` lazy-fetch lands. Undefined for
   *  non-change exchanges. Primitives, so the memo stays cheap. */
  proposedChangeDesc?: string;
  proposedChangeFileCount?: number;
  proposedChangeSummary?: string;
  /** The event a *thread subscription* delivered, resolved once per thread in
   *  `renderExchanges` by following this exchange's
   *  `PromptInjected.delivered_event_id`. Resolved THERE because the target
   *  `EventWaitDelivered` sits in a different exchange. Reading `threadMap` from
   *  this component would undo the memo keeping a 29-exchange thread from
   *  re-parsing every markdown body per SSE event. All undefined unless this
   *  exchange is such a delivery. */
  matchedEventType?: string;
  matchedEventId?: string;
  matchedPayloadJson?: string;
  /** The "Paused by restart" boundary this resume answers, folded in by
   *  `renderExchanges` (see `restartPauseFoldsInto`). Draws nothing in the
   *  panel; the info popover discloses it. A persisted event is immutable, so
   *  the memo compares it by reference. */
  pausedBy?: StoredEvent;
  /** The side questions asked while this turn was the thread's latest, drawn
   *  at their moments in its body (`placeInBody`). Each card object is replaced
   *  when it changes, so the memo compares them by reference. */
  sideQuestions?: readonly SideQuestion[];
}

/**
 * True when a live call row draws its bubble and nothing under it.
 *
 * Three cases, one rule: nothing is in flight BEHIND any of them. A caller
 * mid-sentence has said nothing the engine holds yet, whether the bubble shows
 * a pulse or a partial. The talker's row IS the activity, so a badge under it
 * would be a second one.
 *
 * The caller's FINAL words are the exception the panel exists for. The engine
 * is holding them while the talker decides, so the Requesting shimmer belongs
 * directly under their own turn (ADR 0174).
 */
export function liveRowDrawsNoPanel(userEvent: StoredEvent): boolean {
  if (isLiveReplyRow(userEvent)) return true;
  if (!isLiveUtteranceRow(userEvent)) return false;
  return isLivePartialRow(userEvent) || !(userEvent as { text?: string }).text;
}

/**
 * Does this turn draw the panel naming who started it?
 *
 * Everything does, except a resume (see `isResumeTurn`) and a continuation
 * fragment. A fragment's boundary is older than the page this client holds, so
 * nothing loaded can name it. Its `userEvent` is only its own first step,
 * standing in for a key. Drawing that as an initiator would put a tool call
 * where the reader expects a person.
 *
 * The rows themselves still draw. The head of the turn arrives with the page
 * behind it, and the fold then merges the fragment into the real turn. See
 * `Exchange.continuationFragment`.
 */
export function drawsInitiatorPanel(exchange: Exchange): boolean {
  return exchange.continuationFragment !== true && !isResumeTurn(exchange);
}

/** A resume opens no header of its own. It is a card at the top of the reply it
 *  resumed (`ResumeCard`), so the response panel draws even when that reply is
 *  still empty. */
function isResumeTurn(exchange: Exchange): boolean {
  return exchange.userEvent.type === 'ContinuationStarted';
}


/** The event id to retry when this exchange is an unsent message, else
 *  undefined. A Retry already under way has taken its record, so the card
 *  offers nothing twice. */
function unsentMessageOf(exchange: Exchange): string | undefined {
  if (!isUnsentExchange(exchange)) return undefined;
  const eventId = exchangeStarterId(exchange);
  return eventId && unsentMessages.value.has(eventId) ? eventId : undefined;
}

/** Run `fn` with the control the reader pressed pinned exactly where it is.
 *
 *  The anchor is `currentTarget`, the element carrying this handler, so it IS
 *  the control. A turn control only ever changes heights. The reader asked for
 *  that by pressing one named thing, so that thing is what must not move. See
 *  `withScrollAnchor`.
 *
 *  Read before `fn` runs, because the mutation can take the node away.
 *  `withScrollAnchor` then writes nothing. */
function heldOnThePress(fn: () => void): (e: MouseEvent) => void {
  return (e) => withScrollAnchor(e.currentTarget as HTMLElement | null, fn);
}

function ChatExchangeImpl({ exchange, streamingBuffer, isLast, isQueued, readMarker, threadId, hasPriorActive, priorModel, priorEffort, isContinuableAbort, threadIsCC, threadCodingAgent, threadIdle, threadAwaitingAnswer, behindOpenQuestion, threadCanceling, rowsHidden = 0, proposedChangeDesc, proposedChangeFileCount, proposedChangeSummary, matchedEventType, matchedEventId, matchedPayloadJson, pausedBy, sideQuestions = NO_SIDE_QUESTIONS }: Props) {
  const showDetails = detailsExpanded.value;
  const showSteps = stepsExpanded.value;
  const artifactPaths = loadedOr(artifacts.value, NO_ARTIFACTS);
  const apps = loadedOr(appsList.value, NO_APPS);

  const userMessage = exchangeUserMessage(exchange);
  const userImageHashes = exchangeUserImageHashes(exchange);
  const timestamp = exchangeTimestamp(exchange);
  // A "Read" label dates the read, so the time beside it agrees with it. The
  // origin popover keeps the sent time.
  const readAt = readMarker === 'read' ? messageReadTimestamp(exchange) : undefined;
  const responseTextRaw = exchangeResponseText(exchange);
  const steps = exchangeSteps(exchange, isLast, threadIdle);
  const events = exchangeResponseEvents(exchange, isLast, threadIdle);
  const status = exchangeStatus(exchange, streamingBuffer, isLast, hasPriorActive, threadIsCC, threadIdle, threadAwaitingAnswer, behindOpenQuestion);
  const error = exchangeError(exchange);
  const unsentEventId = unsentMessageOf(exchange);
  // A live turn's output can land between the bubble and this card, so the
  // card names the message Retry would send.
  const unsentText = unsentEventId ? unsentMessages.value.get(unsentEventId)?.body.message.trim() : undefined;

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
  const hasSteps = steps.length > 0 || events.some(e => e.type === 'step');

  // Is there a body to fold, and therefore a body to draw at all? One
  // definition, because `hasBody` on the panel below asks the same thing.
  //
  // Deliberately NOT `hasEvents`. A fold hides the body and lights the control,
  // so on a turn whose body draws nothing it hides nothing and still lights. In
  // flight that is a real case. A coding-agent turn emits a whitespace-only
  // text event before every tool call. A reader with steps off sees nothing
  // else, so `events.length` runs ahead of anything on screen.
  //
  // Asking `drawsResponseRow` instead leaves the control dead exactly while the
  // turn is blank, lighting up with its first drawn row. Turning steps OFF can
  // therefore unfold a step-only turn, which is right: it is showing nothing
  // either way.
  // A resume always draws its card (`ResumeCard`), so it always has a body.
  const isResume = isResumeTurn(exchange);
  const canCollapse = hasResponse || events.some((e) => drawsResponseRow(e, showSteps)) || isResume;
  const isCollapsed = canCollapse && collapsedExchanges.value.has(`${threadId}:${exchange.userSeq}`);
  // Narrower than `isCollapsed`, as `ResponsePanel` is: a headerless turn keeps
  // its body.
  const bodyFolded = canCollapse && turnBodyFolded(collapsedExchanges.value, threadId, exchange);

  const handleLinkClick = (e: MouseEvent) => handleMarkdownLinkClick(e, apps);

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
  // that click leaves the key in the store and the turn folds back up the
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
      pausedBy,
      readAt,
    });
  }

  // Folding this turn, from the header's collapse control. Anchored like the
  // other two: a fold that shrinks the transcript past its own pane clamps the
  // offset, and the reader is owed their control back.
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

  // The body as rows, each told whether the two toggles draw it, so a toggle
  // rolls a row rather than dropping it. A folded turn mounts no body, so it
  // renders no markdown for one. That is also what the render window counts it
  // as (`rowsDrawnByClamp`).
  //
  // The render window's head clamp (`rowsHidden`) applies inside `responseBody`
  // and nowhere earlier. Every verdict above reads the full `events`, since
  // those describe the TURN and must not change because its head is off screen.
  // The open cost is bounded twice, both by ThreadView: which EXCHANGES render,
  // on a step budget, and how many of the FLOOR exchange's rows do, on a row
  // budget (`threadWindow.ts`).
  const sections = useMemo(
    () => (hasEvents && !bodyFolded ? responseBody(events, { showSteps, showDetails, rowsHidden }) : []),
    [hasEvents, bodyFolded, events, showSteps, showDetails, rowsHidden],
  );
  // A body the panel draws (`hasBody`, unfolded) takes its side questions at
  // their moments, or after what it streamed when it has no rows yet. Any
  // other turn draws them after the panel (`bodyTakesCards` below).
  const bodyPieces = useMemo(
    () => placeInBody(sections, canCollapse && !bodyFolded ? sideQuestions : NO_SIDE_QUESTIONS),
    [sections, canCollapse, bodyFolded, sideQuestions],
  );

  // Exactly one running-text shimmer at a time, and it has to be one the reader
  // can SEE. While the live step row is on screen its shimmer is the affordance,
  // so the "Working" label drops to a plain static one. Otherwise the label
  // shimmers as the sole affordance.
  //
  // Two halves, because drawn and seen are different questions. `liveStepInBody`
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
  // A folded turn draws no step, so its label carries the shimmer.
  const liveRowIndex = isCollapsed ? -1 : liveStepInBody(sections);
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
  // Only drawn chunks. A closed row's Disclosure shows the markup it last drew
  // open, so it never reads this map.
  const chunkHtmls = useMemo(() => {
    const map = new Map<ResponseEvent, string>();
    // The placed pieces, not the raw sections: a side question can split a
    // chunk into two rows, each drawn from its own markdown.
    for (const piece of bodyPieces) {
      if (piece.kind !== 'section') continue;
      for (const row of piece.rows) {
        if (row.kind === 'text' && row.open) {
          map.set(row.event, linkifyPaths(renderMarkdown(row.event.md), artifactPaths, apps));
        }
      }
    }
    return map;
  }, [bodyPieces, artifactPaths, apps]);

  const responseHtmlLinkified = useMemo(
    // The streaming buffer's html changes every token, so its linkify opts out
    // of the LRU cache, mirroring `renderMarkdown`'s `cache: false` above.
    // Per-token html would otherwise thrash the cache and evict stable entries.
    () => linkifyPaths(responseHtmlCombined, artifactPaths, apps, { cache: !streamingBuffer }),
    [responseHtmlCombined, artifactPaths, apps, streamingBuffer],
  );

  const responseTerminated = isTerminated(status) || exchange.questionOvertaken === true;

  // Where this turn stands, for a card that asked for work, and the Continue an
  // interrupted response's card offers.
  const boundaryTurn = boundaryTurnOf(status);
  const heldNote = status === 'held' ? HELD_CALLBACK_NOTE : undefined;
  const initiator = useMemo(
    () => withoutActorHeader(
      describeInitiator(exchange, userMessageHtml, userImageHashes, threadId, responseTerminated, threadIsCC, threadCodingAgent, { description: proposedChangeDesc, fileCount: proposedChangeFileCount, summary: proposedChangeSummary }, { eventType: matchedEventType, eventId: matchedEventId, payloadJson: matchedPayloadJson }, heldNote),
      exchange.userEvent,
      {
        turn: boundaryTurn,
        actions: exchange.userEvent.type === 'ResponseAborted' && isContinuableAbort ? <ContinueButton threadId={threadId} /> : undefined,
      },
    ),
    [exchange, userMessageHtml, userImageHashes, threadId, responseTerminated, threadIsCC, threadCodingAgent, proposedChangeDesc, proposedChangeFileCount, proposedChangeSummary, matchedEventType, matchedEventId, matchedPayloadJson, heldNote, boundaryTurn, isContinuableAbort],
  );
  const isChangePanel = isChangeLifecycleEvent(exchange.userEvent);
  // Card-less treatment. A human chat message renders as a right-aligned
  // accent-tinted bubble and change-lifecycle turns render flat. Both drop the
  // actor chip, moving attribution to the clickable timestamp or summary. The
  // predicate is event-type based, NOT label-based. A user-driven control turn
  // keeps the chip slot, rendered iconless with the action AS the label (see
  // `actionInitiator`). Question dividers keep their agent chip.
  const isUserMessageBubble = isUserBubbleEvent(exchange.userEvent) && initiator.variant === 'user';
  // A speech-only turn joins them: it is two people talking, so neither half of
  // it gets a header naming who spoke. The talker's greeting is the card this
  // changes, the caller's own bubbles being chromeless already.
  const isSpeechOnly = isSpeechOnlyTurn(exchange);
  const isChromeless = isUserMessageBubble || isChangePanel || isSpeechOnly || initiator.chromeless === true;
  // Every chipless turn is exempt from the fold, on report. A change turn's body
  // is a summary, a description and a file list; a user message is the reader's
  // own text. The control cost a row of chrome to fold a few short lines.
  const canCollapseInitiator = !isChromeless
    && (!!initiator.summary || !!initiator.details);
  const isInitiatorCollapsed = canCollapseInitiator
    && collapsedInitiators.value.has(`${threadId}:${exchange.userSeq}`);
  const changeId = exchangeChangeId(exchange, isChangePanel, threadIsCC);
  // A queued follow-up shows a "Queued" tag in its own bubble header, where
  // dividers show "Answered". A faux "Lucidos Agent" response panel below it
  // would misattribute it: the message is the user's, and a stack of them
  // should each read as waiting.
  const isQueuedUserMessage = !!isQueued && isUserMessageBubble;
  // What waits behind an open question dims. A queued message carries no note:
  // its own "Queued" says it. A callback does, unless its card drew it already.
  const panelHeld = isQueuedUserMessage && threadAwaitingAnswer ? {}
    : heldNote && !initiator.heldNoteInCard ? { note: heldNote } : undefined;
  // Claude Code can take back a message it has not read; Codex cannot.
  const canTakeBack = !threadIsCC || threadCodingAgent === 'claude-code';
  const queuedMessageId = isQueuedUserMessage && canTakeBack ? exchange.userEvent._eventId : undefined;
  const isLiveRow = liveRowDrawsNoPanel(exchange.userEvent);
  // The buttons live INSIDE the status label, an existing `display: flex` row,
  // rather than in a separate wrapper. "Queued" and its buttons then stay on
  // one line using only CSS that already ships.
  const queuedStatus = (
    <span class="exchange-status-label exchange-status-queued">
      {'Queued'}
      {queuedMessageId && (
        <>
          <button
            type="button"
            class="icon-btn inline-icon queued-message-edit exchange-status-glyph"
            aria-label="Edit queued message"
            data-tooltip="Edit queued message"
            onClick={(e) => {
              e.stopPropagation();
              void editQueuedMessage(threadId, {
                id: queuedMessageId,
                text: exchangeUserMessage(exchange),
                imageHashes: exchangeUserImageHashes(exchange),
              });
            }}
          >
            <EditIcon />
          </button>
          <button
            type="button"
            class="icon-btn inline-icon queued-message-remove exchange-status-glyph"
            aria-label="Remove queued message"
            data-tooltip="Remove queued message"
            onClick={(e) => {
              e.stopPropagation();
              void removeQueuedMessage(threadId, queuedMessageId);
            }}
          >
            <TrashIcon />
          </button>
        </>
      )}
    </span>
  );
  const readStatus = readMarker && !isQueuedUserMessage && (
    <span class={`exchange-status-label exchange-status-${readMarker}`}>
      {readMarker === 'read' ? 'Read' : 'Sent'}
    </span>
  );
  const initiatorStatus = isQueuedUserMessage || readStatus
    ? <>{initiator.status}{isQueuedUserMessage && queuedStatus}{readStatus}</>
    : undefined;
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
  //
  // A stopped child's note on its parent is the same shape (ADR 0252): it
  // wakes nothing, so no response ever follows it.
  const isTurnlessPanel = isTurnlessBoundary(exchange.userEvent);
  // A speech-only turn draws its panel for the WORDS alone. The header is the
  // only thing a status badge sits on. So with no reply yet, the panel would be
  // an empty box under the caller's bubble.
  const speechOnlyHasWords = !isSpeechOnly || canCollapse;
  const showResponsePanel = status !== 'held' && (isResume || (!isChangePanel || isChangeContinuation) && (!isAbortPanel || isTerminatedContinuation) && (!isCancelPanel || isTerminatedContinuation) && !isTurnlessPanel && !isUnansweredDivider && !isEmptyContinued && !isQueuedUserMessage && !isLiveRow && speechOnlyHasWords && (hasResponse || hasEvents || showStatus));
  // The panel draws its body only when it has one and is unfolded, which is
  // exactly when `bodyPieces` carries the cards.
  const bodyTakesCards = showResponsePanel && canCollapse && !bodyFolded;
  const executor = describeExecutor(threadIsCC, threadCodingAgent);

  // Keyed by the row's index in the whole turn (`responseBody`). So no toggle,
  // no head clamp and no new chunk remounts a row that stays drawn.
  function renderRow(row: BodyRow) {
    switch (row.kind) {
      case 'text':
        // Classed so the chunk can own the space around itself. Interleaved
        // with step rows, a markdown paragraph's bottom-only margin is all that
        // separates prose from a log row, putting the air on one side. See
        // `.response-chunk` in chat/response.css.
        return (
          <Disclosure key={`t${row.key}`} open={row.open}>
            <ReplyHtml
              class="response-chunk"
              textSeq={row.event.seq}
              html={chunkHtmls.get(row.event) ?? ''}
              final
              threadId={threadId}
            />
          </Disclosure>
        );
      case 'steps':
        // The live row is marked so the header label can read where it sits.
        // MOVING the mark is safe in either direction. Preact clears the old ref
        // during the diff and applies the new one after it. So a clear can never
        // land on top of a set. Losing the mark entirely is the case Preact does
        // not handle, and the reader above gates on the index for exactly that.
        //
        // The hairline for hidden steps rolls in as the run rolls out, so the
        // gap between the two chunks never jumps when either roll lands. The
        // pair is keyed as one: an unkeyed group matches its neighbours by
        // position and remounts both rows when the head clamp shifts.
        return (
          <Fragment key={`s${row.key}`}>
            <Disclosure open={row.open}>
              {row.steps.map(({ event, index }) => {
                const rowRef = index === liveRowIndex ? setLiveStepRow : undefined;
                return event.children?.length
                  ? <SubAgentStepGroup key={index} event={event} rowRef={rowRef} />
                  : <InlineStep key={index} event={event} rowRef={rowRef} />;
              })}
            </Disclosure>
            <Disclosure open={row.elided}>
              <div class="response-elision" aria-hidden="true" />
            </Disclosure>
          </Fragment>
        );
      case 'marker':
        return renderMarker(row.event, row.key);
    }
  }

  function renderMarker(evt: ResponseEvent, k: number) {
    if (evt.type === 'image') return <GeneratedImage key={`img${k}`} event={evt} />;
    if (evt.type === 'checkpoint') return <CheckpointCard key={`cp${k}`} event={evt} />;
    if (evt.type === 'widget') {
      return (
        <WidgetCard
          key={`wg${k}`}
          threadId={threadId}
          appId={evt.app_id}
          params={evt.params}
          label={evt.label}
          eventId={evt.event_id}
        />
      );
    }
    // Ungated, like the event row below. It is what the caller HEARD, no
    // audio is kept, and the written answer beside it is a different thing:
    // the talker says what an answer means rather than reading it out.
    if (evt.type === 'spoken_reply') return <SpokenReply key={`sr${k}`} event={evt} />;
    // Ungated, like every other marker. The park is the transcript's only
    // record that the thread subscribed to something. The clock indicator
    // holds the LIVE half and drops the wait as it resolves. A toggle
    // defaulting to off would leave a resolved wait recorded nowhere.
    if (evt.type === 'event_wait') return <EventWaitRow key={`ew${k}`} event={evt} />;
    // Ungated too: a held message is the user's cue that a reply is owed.
    if (evt.type === 'held_message') return <HeldMessageRow key={`hm${k}`} event={evt} />;
    // Ungated: an open one is how the user reaches a form they closed or
    // never saw, and a resolved one records how the request ended.
    if (evt.type === 'form_request') return <FormRequestRow key={`fr${k}`} row={evt} threadId={threadId} />;
    if (evt.type === 'empty') return <div key={`e${k}`} class="response-empty-note">{'The agent finished without writing a reply. Ask it to sum up what it did.'}</div>;
    return null;
  }

  // Identity for keyboard turn-nav's Enter toggle (see
  // scrollState.toggleNavigatedTurnCollapsed): which collapse store the ⌘↑/⌘↓-
  // highlighted turn folds. Response body wins; a response-less divider/change turn
  // falls back to its initiator panel; absent when neither is collapsible so Enter
  // is a no-op there.
  //
  // A speech-only turn folds neither half. Both its panels draw headerless, so
  // there is no collapse control to unfold a fold with.
  const collapseKind = isSpeechOnly ? undefined
    : canCollapse ? 'response'
      : canCollapseInitiator ? 'initiator' : undefined;

  return (
    <div class="chat-exchange" data-event-id={exchangeStarterId(exchange)} data-change-id={changeId || undefined}
         data-thread-id={threadId} data-user-seq={exchange.userSeq} data-collapse-kind={collapseKind}
         data-head-clamped={rowsHidden > 0 ? '' : undefined}>
      {/* Keyed, because a fragment omits this one and both panels animate.
          Preact does hold the slot an `&&`-guarded child leaves behind, so
          index matching survives this particular omission on its own. The keys
          are what keep that true if the guard ever becomes an early return of
          a different tree (`.claude/rules/frontend.md`). */}
      {drawsInitiatorPanel(exchange) && (
      <InitiatorPanel
        key="initiator"
        initiator={initiatorStatus
          // Appended, not replaced. A spoken message already put its own chip
          // in this slot, and both facts are true of a queued utterance.
          ? { ...initiator, status: initiatorStatus }
          : initiator}
        timestamp={formatMessageTimestamp(readAt ?? timestamp)}
        onActorClick={initiator.actorClickable === false || !originPopoverHasContent(exchange)
          ? undefined
          : (e) => openInfoPanel('origin', e)}
        // The same router the response body gets. This panel renders markdown
        // too, so its links owe the reader the same routing and the same
        // terminal guard.
        onBodyClick={handleLinkClick}
        bubble={isUserMessageBubble}
        chromeless={isChromeless}
        collapsible={canCollapseInitiator}
        collapsed={isInitiatorCollapsed}
        onToggle={canCollapseInitiator ? toggleInitiator : undefined}
        held={panelHeld}
      />
      )}

      {showResponsePanel && (
        <ResponsePanel
          key="response"
          executor={executor}
          onExecutorClick={(e) => openInfoPanel('executor', e)}
          controls={turnControlsSlot}
          headerless={isSpeechOnly}
          hasBody={canCollapse}
          status={showStatus && shouldShowResponseStatusBadge(exchange.userEvent, statusClass) ? (
            <span class={`exchange-status-label exchange-status-${statusClass}`} data-tooltip={sl.tooltip}>
              {/* The active status label — Working / Requesting / Canceling —
                  shimmers as the AI running-text affordance, which replaces the
                  spinner (no mini-spinner in the 'working' state). Suppressed
                  when a live step is already shimmering on screen, so only one
                  running-text affordance moves at a time (see liveStepOnScreen). */}
              {/* The arrow rides inside the word, inline, so it sits on the
                  word's own baseline rather than on the row's centre line. */}
              <span class={statusClass === 'working' && !liveStepOnScreen ? 'running-shimmer' : undefined}>
                {statusLabelText}
                {status === 'interrupted' && <ContinuedIcon className="exchange-status-continued exchange-status-glyph" />}
              </span>
              {statusClass === 'waiting' && <span class="progress-dot progress-dot-waiting exchange-status-glyph" />}
              {statusClass === 'canceled' && <span class="exchange-status-x exchange-status-glyph">{'✕'}</span>}
              {statusClass === 'error' && <span class="exchange-status-x exchange-status-glyph">{'✕'}</span>}
              {statusClass === 'aborted' && <WarningIcon className="exchange-status-warning exchange-status-glyph" />}
            </span>
          ) : null}
          timestamp={formatMessageTimestamp(responseTimestamp || timestamp)}
          collapsed={isCollapsed}
        >
          {/* One keyed list for both shapes, so a card under streamed text
              stays mounted when the first row replaces that text. */}
          {[
            // In `.response-content`, as every card in a body is, so it takes
            // the same left inset as the cards and text around it.
            ...(isResume ? [<div class="response-content" key="resume"><ResumeCard exchange={exchange} /></div>] : []),
            ...(hasEvents ? [] : [
              <div class="response-content markdown-content" key="streamed" onClick={handleLinkClick}>
                <ReplyHtml html={responseHtmlLinkified} final={!streamingBuffer} threadId={threadId} />
              </div>,
            ]),
            ...bodyPieces.map((piece) => (piece.kind === 'cards'
              ? <SideQuestionGroup key={piece.key} items={piece.items} />
              : (
                <div class="response-content markdown-content" key={`sec-${piece.key}`} onClick={handleLinkClick}>
                  {piece.rows.map(renderRow)}
                </div>
              ))),
          ]}
          {isEngineLimit && (
            <div class="exchange-engine-limit" role="status" onClick={handleLinkClick}>
              <strong>{engineLimitHeading(engineLimitDetail)}</strong>
              {/* Markdown, so the Settings link in the engine's text is a link. */}
              <div dangerouslySetInnerHTML={{ __html: linkifyPaths(renderMarkdown(engineLimitDetail), artifactPaths, apps) }} />
            </div>
          )}
        </ResponsePanel>
      )}

      {!bodyTakesCards && sideQuestions.length > 0 && (
        <SideQuestionGroup key="side-questions:after:turn" items={sideQuestions} />
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
          <strong>{unsentEventId ? 'Not sent' : 'The reply failed'}</strong>
          {unsentText && <blockquote class="exchange-error-quote">{unsentText}</blockquote>}
          <p>{error.message}</p>
          {unsentEventId
            ? (
              <div class="exchange-error-actions">
                <button type="button" class="action-btn action-btn-secondary exchange-error-discard" onClick={() => discardUnsentMessage(unsentEventId)}>
                  Discard
                </button>
                <button type="button" class="action-btn exchange-error-retry" onClick={() => void retryUnsentMessage(unsentEventId)}>
                  Retry
                </button>
              </div>
            )
            : <p>Send a message to try again.</p>}
        </div>
      )}
    </div>
  );
}

/** How the engine's tool-call cap message opens (`tool_call_cap_message` in
 *  `agentic_loop/helpers.rs`), once its `[ENGINE-LIMIT]` prefix is cut. */
export const TOOL_CALL_CAP_OPENING = 'Per-turn limit';

/** The turn-limit card's heading. The engine writes two limit messages, and
 *  only the tool-call one means a cap the user set was reached. */
function engineLimitHeading(detail: string): string {
  return detail.startsWith(TOOL_CALL_CAP_OPENING) ? 'Tool-call limit reached' : 'Lucidos ended this turn';
}

/** Are two mark sets the same? Absent and empty are one state: a resolution
 *  deletes the last entry rather than dropping the set, so the two spellings
 *  of "nothing is marked" must not read as a change.
 *
 *  Iterated rather than compared by identity, because a FULL rebuild allocates
 *  fresh sets. See the `blockedStepSeqs` line in `chatExchangePropsEqual`. */
function sameMarks<T>(a: Set<T> | undefined, b: Set<T> | undefined): boolean {
  if (a === b) return true;
  if (!a || !b) return (a?.size ?? 0) === (b?.size ?? 0);
  if (a.size !== b.size) return false;
  for (const mark of a) if (!b.has(mark)) return false;
  return true;
}

/** The same cards, each unchanged. A card is replaced whenever its state
 *  moves, so reference equality per card is exact. */
function sameSideQuestions(a: readonly SideQuestion[] = NO_SIDE_QUESTIONS, b: readonly SideQuestion[] = NO_SIDE_QUESTIONS): boolean {
  return a.length === b.length && a.every((card, i) => card === b[i]);
}

/** The words in the row's own bubble, for the fingerprint below.
 *
 *  Both types a LIVE call row can wear. The caller's is a `MessageReceived`
 *  and the talker's a `SpokenReplyGenerated`, and each is rewritten in place as
 *  the words arrive. A persisted event of either type is immutable, so this
 *  compare costs nothing everywhere else. */
function userBubbleText(exchange: Exchange): string | undefined {
  const ev = exchange.userEvent;
  if (ev.type === 'MessageReceived') return ev.text;
  return ev.type === 'SpokenReplyGenerated' ? ev.text : undefined;
}

/** The unsent message a question card's typed answer would retry, if any. */
function unsentEventIdOf(exchange: Exchange): string | undefined {
  return exchange.typedAnswer?.state === 'unsent' ? exchange.typedAnswer.unsentEventId : undefined;
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
 *   - `liveReplyText`, the talker's live row growing word by word. Its step
 *     moves neither of the two terms above, so nothing else here sees it.
 *   - `questionOvertaken`, flipped when the agent ignored a question.
 *   - `continuationMoved`, the turn handed to a later exchange, which
 *     finalizes this one's pending Thinking marker.
 *   - `releasedFromHold`, which a rebuild can settle differently once a late
 *     release arrives. It changes the header text.
 *   - `typedAnswer`, the composer's answer drawn on a question card until the
 *     engine confirms it, sending or not sent.
 *   - `blockedStepSeqs` / `deniedStepSeqs`, a permission decision on a call
 *     this exchange owns, and `deliveredHeldIds`. These marks are written from
 *     OUTSIDE, by a later exchange, so nothing else here moves with them.
 *
 *  All other props are primitives or strings, compared with Object.is. */
export function chatExchangePropsEqual(prev: Props, next: Props): boolean {
  if (prev.revision !== next.revision) return false;
  if (prev.streamingBuffer !== next.streamingBuffer) return false;
  if (prev.isLast !== next.isLast) return false;
  if (prev.isQueued !== next.isQueued) return false;
  if (prev.readMarker !== next.readMarker) return false;
  if (prev.threadId !== next.threadId) return false;
  if (prev.hasPriorActive !== next.hasPriorActive) return false;
  if (prev.priorModel !== next.priorModel) return false;
  if (prev.priorEffort !== next.priorEffort) return false;
  if (prev.isContinuableAbort !== next.isContinuableAbort) return false;
  if (prev.threadIsCC !== next.threadIsCC) return false;
  if (prev.threadCodingAgent !== next.threadCodingAgent) return false;
  if (prev.threadIdle !== next.threadIdle) return false;
  if (prev.threadAwaitingAnswer !== next.threadAwaitingAnswer) return false;
  if (prev.behindOpenQuestion !== next.behindOpenQuestion) return false;
  if (prev.threadCanceling !== next.threadCanceling) return false;
  // Without this the memo swallows every scroll-up round into the floor turn:
  // the window grows, nothing else about the turn changes, and the head the
  // reader scrolled up for never draws.
  if (prev.rowsHidden !== next.rowsHidden) return false;
  if (prev.proposedChangeDesc !== next.proposedChangeDesc) return false;
  if (prev.proposedChangeFileCount !== next.proposedChangeFileCount) return false;
  if (prev.proposedChangeSummary !== next.proposedChangeSummary) return false;
  if (prev.matchedEventType !== next.matchedEventType) return false;
  if (prev.matchedEventId !== next.matchedEventId) return false;
  if (prev.matchedPayloadJson !== next.matchedPayloadJson) return false;
  if (prev.pausedBy !== next.pausedBy) return false;
  if (!sameSideQuestions(prev.sideQuestions, next.sideQuestions)) return false;
  const a = prev.exchange;
  const b = next.exchange;
  if (a.userSeq !== b.userSeq) return false;
  // The one row whose TEXT moves under a stable identity: the caller's own
  // bubble, rewritten the instant the provider transcribes it (ADR 0174).
  // Every other term here holds across that swap, so without this the memo
  // keeps drawing the pulse over words the reader could be reading. A
  // persisted event is immutable and identity-stable, so this compare is a
  // reference check everywhere else.
  if (userBubbleText(a) !== userBubbleText(b)) return false;
  // The talker's live row is the same shape one step down. It is a STEP, so
  // the count and the last seq below hold across every word it gains. Without
  // this the bubble stops on whatever prefix the first render caught.
  if (a.liveReplyText !== b.liveReplyText) return false;
  // `continuationFragment` is deliberately absent. A fragment keys off its first step, and
  // the turn it merges into keys off its boundary. So the page that clears the
  // flag also changes `exchangeKey` and remounts the node, and this compare
  // could only ever answer with itself.
  if (a.questionOvertaken !== b.questionOvertaken) return false;
  if (a.continuationMoved !== b.continuationMoved) return false;
  if (a.releasedFromHold !== b.releasedFromHold) return false;
  // A typed answer comes, changes state and goes on a clone with the same steps.
  if (a.typedAnswer?.text !== b.typedAnswer?.text) return false;
  if (a.typedAnswer?.state !== b.typedAnswer?.state) return false;
  if (unsentEventIdOf(a) !== unsentEventIdOf(b)) return false;
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
  if (!sameMarks(a.blockedStepSeqs, b.blockedStepSeqs)) return false;
  if (!sameMarks(a.deniedStepSeqs, b.deniedStepSeqs)) return false;
  // Same shape: a held message's delivered copy, a later exchange, hides the
  // held row here.
  if (!sameMarks(a.deliveredHeldIds, b.deliveredHeldIds)) return false;
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
// description (Hardening needed, Change applied, Wait timed out). Rich
// payloads (message text, change description, file list) go in `details`.
// Click the actor to open the route popover for finer origin info.
// ---------------------------------------------------------------------------

type InitiatorVariant = 'user' | 'system' | 'trigger' | 'lucidos';

/** What the thread already says about a change, for its lifecycle card. */
export interface ProposedChangeSeed {
  description?: string;
  fileCount?: number;
  summary?: string;
}

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
  /** Drop the actor chip, as a user bubble and a change card do. Set by
   *  `withoutActorHeader` on every turn the agent did not write. */
  chromeless?: boolean;
  /** The body's event card draws the held note itself, so the panel adds no
   *  note and no dim of its own. */
  heldNoteInCard?: boolean;
}

/** Action label shared by the panel header and the route popover's Origin row. */
function initiatorSummary(exchange: Exchange): string {
  const ev = exchange.userEvent;
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
    case 'MissingHardeningDetected': return 'Hardening needed';
    case 'MergeConflictDetected':    return 'Merge conflict';
    case 'CodingAgentPromptSent':    return 'Instructions from Lucidos';
    // No summary line: the change event row carries the description and a state badge.
    case 'ChangeApplied':
    case 'ChangeDiscarded':
    case 'ChangeReverted':
    case 'ChangeApplyFailed':        return '';
    case 'PromptInjected':           return injectedMessageSummary(ev);
    case 'EventWaitCanceled':        return eventWaitStoppedSummary(ev.reason);
    case 'MessageReceived': {
      if (ev.origin?.kind === 'api') return 'API message';
      if (modeToInitiator(ev.mode) !== 'system') return '';
      // An engine-seeded thread names what it is about, as the popover does.
      const headline = ev.origin?.kind === 'engine' ? engineReasonHeadline(ev.origin.reason) : null;
      if (headline) return `${headline.label}: ${headline.value}`;
      // The chip says only "Lucidos Agent", so the summary names the sender.
      const sender = agentMessageSender(ev.origin);
      const said = sender ? `Message from ${sender}` : 'Message from an agent';
      return exchange.releasedFromHold ? `${said}, held until you replied` : said;
    }
    // Divider exchanges — the body component carries the question/permission
    // text, so the panel needs no separate summary line.
    case 'UserQuestionAsked':            return '';
    case 'CodingAgentPermissionRequest': return '';
    case 'McpConsentRequested':          return `Tool consent requested: ${ev.tool}`;
    case 'ChildThreadCompleted':         return '';
    case 'ChildThreadStopped':           return '';
    case 'ChildThreadDetached':          return '';
    default:                         return '';
  }
}

/** Who handed the running agent these words, and why, in one line. */
function injectedMessageSummary(ev: Extract<StoredEvent, { type: 'PromptInjected' }>): string {
  const reentry = waitReentryReason(ev);
  if (reentry) return reentry.outcome === 'expired' ? 'Wait timed out' : 'Event arrived';
  if (ev.origin?.kind === 'device') return 'Your message, read while the agent worked';
  const sender = agentMessageSender(ev.origin);
  return sender ? `Message from ${sender}` : 'Message from Lucidos';
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

/** Only agent output wears a header. A turn the agent did not write (the
 *  engine's, the system's, or your own control press) draws chromeless, like a
 *  change card: its time, which opens the route popover, above a card. A
 *  starter that said only a line of text gets that line as its card.
 *
 *  Applied where a turn is drawn rather than inside `describeInitiator`, so the
 *  descriptor keeps naming the actor and the summary for everything else. */
function withoutActorHeader(
  d: InitiatorDescriptor,
  starter: StoredEvent,
  card: { turn: BoundaryTurn; actions?: ComponentChildren },
): InitiatorDescriptor {
  const narrated = d.label === ENGINE_LABEL || d.label === SYSTEM_LABEL || d.icon === null;
  if (!narrated) return d;
  // An action descriptor carries its words as the label (`actionInitiator`).
  const summary = d.summary ?? (d.icon === null ? d.label : undefined);
  if (!summary) return { ...d, chromeless: true };
  return { ...d, chromeless: true, summary: undefined, details: boundaryCard(starter, summary, { carried: d.details, ...card }) };
}

/** Where a turn stands, as a boundary card reports it. */
function boundaryTurnOf(status: ExchangeStatus): BoundaryTurn {
  if (isStatusActive(status) || status === 'awaiting-answer') return 'working';
  if (status === 'done') return 'done';
  if (status === 'queued' || status === 'held') return 'waiting';
  return 'stopped';
}

/** Build a descriptor in the "Response canceled" style: no icon, the action
 *  text AS the label, and no separate summary line. The label chip opens the
 *  origin popover whenever it has something to add (`originPopoverHasContent`).
 *  Shared by every user-driven control turn (Restart, Continue, auto-prompt,
 *  credential/consent) so they
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
 *  - "Answered" or "Resolved" when the user responded, and who settled it
 *    when Lucidos did (`permissionResolvedLabel`).
 *  - "Canceled" (✕) when they dismissed it.
 *  - "Unanswered" or "Unresolved" when the turn ended for any other reason.
 *  - "Sending" from the user's pick until the engine confirms it.
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
  pick: PendingPick,
): ComponentChildren {
  if (resolved) return <span class="exchange-status-label exchange-status-done">{resolvedLabel}</span>;
  if (terminal === 'canceled') return <span class="exchange-status-label exchange-status-canceled">{'Canceled'}<span class="exchange-status-x exchange-status-glyph">{'✕'}</span></span>;
  // Neutral, like a Codex follow-up redirect: the user steered, they did not
  // dismiss. A "Canceled ✕" here would blame them for a question they replied
  // past.
  if (terminal === 'superseded') return <span class="exchange-status-label exchange-status-dropped">{'Superseded'}</span>;
  if (terminal === 'dropped') return <span class="exchange-status-label exchange-status-dropped">{droppedLabel}</span>;
  return <AwaitingStatus {...pick} />;
}

/** Where a divider's card keeps its optimistic pick, and the card's id there.
 *  `typed` is the state of a question card's typed answer, which the exchange
 *  carries rather than the picks. `unsent` holds a question card's picks that
 *  were not sent. */
type PendingPick = {
  picks: ReadonlySignal<ReadonlyMap<string, unknown>>;
  id: string;
  typed?: TypedAnswer['state'];
  unsent?: ReadonlySignal<ReadonlyMap<string, unknown>>;
};

/** A component, not a plain span, so it reads the pick itself. The descriptor
 *  that holds it is memoized, so a signal read there would not re-render. */
export function AwaitingStatus({ picks, id, typed, unsent }: PendingPick) {
  if (picks.value.has(id) || typed === 'sending') {
    return <span class="exchange-status-label exchange-status-sending">{'Sending'}</span>;
  }
  if (typed === 'unsent' || unsent?.value.has(id)) return <span class="exchange-status-label exchange-status-not-sent">{'Not sent'}</span>;
  return <span class="exchange-status-label exchange-status-awaiting">{'Needs your answer'}</span>;
}

/** A permission step's verdict, in the one shape every permission card reads. */
type PermissionVerdict = Pick<ResolvedPermission, 'allowed' | 'reason' | 'persist_scope'>;

function permissionVerdict(step: PermissionVerdict | undefined): PermissionVerdict | undefined {
  return step && { allowed: step.allowed, reason: step.reason, persist_scope: step.persist_scope };
}

/** A resolved permission card's header word. A card nobody answered must not
 *  read as answered: one Lucidos allowed says so, and one closed unanswered
 *  says Closed, with the note under it saying why. */
function permissionResolvedLabel(verdict: PermissionVerdict | undefined): string {
  if (!verdict || !engineResolutionNote(verdict)) return 'Resolved';
  return verdict.allowed ? 'Allowed by Lucidos' : 'Closed';
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
  /** The in-thread seed for <ChangeEventRow> on the change-lifecycle arms, so
   *  the body paints at full height on first open. One object, since three
   *  same-typed positional neighbours mis-order with no type error. */
  proposedChange: ProposedChangeSeed = {},
  /** The event a *thread subscription* delivered, already resolved through
   *  this exchange's `PromptInjected.delivered_event_id` (see
   *  `buildDeliveredEventInfo`). Undefined for every exchange that is not such
   *  a delivery.
   *
   *  ONE object rather than three trailing `string | undefined` params. At the
   *  end of a twelve-argument positional list, same-typed neighbours mis-order
   *  with no type error, and inserting one silently re-binds a caller's
   *  argument. The fields are still flat primitives, never the payload object,
   *  so `chatExchangePropsEqual` compares them without a deep walk. */
  matched?: { eventType?: string; eventId?: string; payloadJson?: string },
  /** Set while this turn waits behind an open question. An arm whose card can
   *  carry it passes it in and sets `heldNoteInCard`. */
  heldNote?: string,
): InitiatorDescriptor {
  const ev = exchange.userEvent;
  // Ahead of the switch, because the row wears a `MessageReceived` and would
  // otherwise take that arm's origin reasoning on a row that has no origin.
  //
  // Two shapes, one row. While the caller is speaking there are no words, so
  // the bubble holds a pulse where they will go. Once the provider ends the
  // turn the words are there. The bubble is then the one a spoken message
  // gets, because that is what the row now is. The swap costs no round trip
  // and no frame (ADR 0174).
  if (isLiveUtteranceRow(ev)) {
    const partial = isLivePartialRow(ev) ? (ev as { text?: string }).text ?? '' : null;
    return youInitiator({
      details: partial !== null
        ? <LivePartialBody text={partial} />
        : userMessageHtml
          ? <UserMessageBody html={userMessageHtml} imageHashes={[]} />
          : <LiveUtteranceBody />,
      status: <SpokenChip />,
    });
  }
  // A *continuation fragment* has no starter to describe: the boundary that
  // opened its turn is older than the loaded page, and its `userEvent` is only
  // its own first step. Nothing draws this, `drawsInitiatorPanel` suppressing
  // the panel, but the descriptor still reaches `isUserMessageBubble` and
  // `canCollapseInitiator`. Falling to the default below hands an agent's turn
  // the reader's own "You" chip, which is what those two would then read.
  if (exchange.continuationFragment) return { variant: 'system', icon: null, label: '' };
  const summary = initiatorSummary(exchange);
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
      // Draws no panel (`drawsInitiatorPanel`): `ResumeCard` shows the resume.
      // The descriptor still names who resumed, for the route popover. A
      // device-driven continue carries the action as its label, like a cancel.
      if (ev.actor?.kind === 'device') return actionInitiator(summary);
      return { variant: actorVariant(ev.actor), ...actorInitiator(ev.actor), summary };
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
    case 'ChangeApplyFailed':
      return {
        variant: 'system', accent: changeAccent(ev.type),
        ...actorInitiator(ev.actor),
        details: (
          <ChangeEventRow
            type={ev.type}
            changeId={ev.change_id}
            error={ev.type === 'ChangeApplyFailed' ? ev.error : undefined}
            seedDescription={proposedChange.description}
            seedFileCount={proposedChange.fileCount}
            seedSummary={proposedChange.summary}
          />
        ),
      };
    case 'PromptInjected':
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
        // No summary line on the delivery: its event row already names the
        // event and says "Arrived", so a header saying the same prints it twice.
        // Same as the trigger and the child callback, whose rows own their
        // prefixes too.
        summary: matched?.eventType ? undefined : summary,
        heldNoteInCard: !!heldNote && !!matched?.eventType,
        details: matched?.eventType
          ? <EventDeliveryBody eventType={matched.eventType} eventId={matched.eventId} payloadJson={matched.payloadJson} heldNote={heldNote} />
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
    //
    // The talker's LIVE row wears this type too, and takes this arm with it.
    // One shape for a reply being said and for the same reply written down.
    // The swap to the engine's own row then moves nothing on screen.
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
        heldNoteInCard: !!heldNote,
        details: (
          <ChildCompletionRow
            childThreadId={ev.child_thread_id}
            childThreadTitle={ev.child_thread_title}
            status={ev.status}
            summary={ev.summary}
            pendingChangeIds={ev.pending_change_ids}
            subThreadPendingChanges={ev.sub_thread_pending_changes}
            heldNote={heldNote}
          />
        ),
      };
    case 'ChildThreadStopped':
      // The same fan-in raises this in place of the completion card when a
      // user Stop pauses the child (ADR 0252). Engine plumbing, attributed
      // like its sibling above.
      return {
        variant: 'system',
        icon: <LucidosGlyph />,
        label: ENGINE_LABEL,
        actorClickable: false,
        details: (
          <ChildStoppedRow
            childThreadId={ev.child_thread_id}
            childThreadTitle={ev.child_thread_title}
          />
        ),
      };
    case 'ChildThreadDetached':
      // One of this thread's children moved to top level (ADR 0278). A note on
      // the former parent, attributed like its siblings above.
      return {
        variant: 'system',
        icon: <LucidosGlyph />,
        label: ENGINE_LABEL,
        actorClickable: false,
        details: (
          <ChildMovedOutRow
            childThreadId={ev.child_thread_id}
            childThreadTitle={ev.child_thread_title}
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
          { picks: pendingAnswers.map, id: ev.tool_use_id, typed: exchange.typedAnswer?.state, unsent: unsentPicks.map },
        ),
        details: (
          <QuestionBody
            threadId={threadId}
            toolUseId={ev.tool_use_id}
            question={ev.question}
            options={ev.options ?? []}
            multiSelect={ev.multi_select}
            resolved={answered?.answer}
            typedAnswer={exchange.typedAnswer}
            terminated={responseTerminated}
          />
        ),
      };
    }
    case 'CodingAgentPermissionRequest': {
      const resolvedStep = findPermissionResolution(exchange, ev.request_id);
      const resolved = permissionVerdict(resolvedStep);
      const agent = describeExecutor(true, threadCodingAgent);
      return {
        variant: 'lucidos',
        icon: agent.icon,
        label: agent.label,
        status: dividerStatus(!!resolvedStep, permissionResolvedLabel(resolved), 'Unresolved', responseTerminated ? 'dropped' : null, { picks: pendingVerdicts.map, id: ev.request_id }),
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
      const resolved = permissionVerdict(resolvedStep);
      // The command guard only fires on chat threads → the Lucidos Agent.
      const agent = describeExecutor(false);
      return {
        variant: 'lucidos',
        icon: agent.icon,
        label: agent.label,
        status: dividerStatus(!!resolvedStep, permissionResolvedLabel(resolved), 'Unresolved', responseTerminated ? 'dropped' : null, { picks: pendingVerdicts.map, id: ev.request_id }),
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
      const resolved = permissionVerdict(resolvedStep);
      // The chat MCP permission lane only fires on chat threads → Lucidos Agent.
      const agent = describeExecutor(false);
      return {
        variant: 'lucidos',
        icon: agent.icon,
        label: agent.label,
        status: dividerStatus(!!resolvedStep, permissionResolvedLabel(resolved), 'Unresolved', responseTerminated ? 'dropped' : null, { picks: pendingVerdicts.map, id: ev.request_id }),
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
    case 'McpConsentRequested':
      // Iconless action label (ResponseCanceled style). No answer is ever
      // recorded, so its chip opens no popover. No body component: nothing
      // emits it today.
      return actionInitiator(summary);
    default:
      // Unreachable in production (groupIntoExchanges only assigns starter
      // types to userEvent), but `userEvent: StoredEvent` covers every event
      // variant for legacy reasons, so TS can't enforce exhaustiveness here.
      return youInitiator();
  }
}

export { describeExecutor } from './chat-exchange-parts';
