import { turnDeviceName } from '../../store/actions/devices';
import { spendCapResetLabel } from '../../utils/spendCap';
import { providerLabel } from '../../store/modelProviders';
import { Fragment, type ComponentChildren } from 'preact';
import { InlineWidget } from '../widgets/InlineWidget';
import { ReplyHtml } from '../widgets/useWidgetEmbedSlots';
import type { ReadonlySignal, Signal } from '@preact/signals';
import { memo } from 'preact/compat';
import { useMemo, useState } from 'preact/hooks';
import { loadedOr } from '../../store/types';
import type { ResponseEvent, App } from '../../store/types';
import type { CodingAgent } from '../../api/types';
import type { Exchange, ReadMarker, StoredEvent, ThreadEvent, MessageOrigin, ResolvedPermission, TypedAnswer } from '../../store/thread-events';
import { ENGINE_LABEL, SYSTEM_LABEL, API_CALLER_LABEL, unattestedCallerLabel, LUCIDOS_AGENT_LABEL, abortPromisesAutoResume, drawsInitiatorPanel, isResumeTurn, exchangeUserMessage, isUserBubbleEvent, exchangeUserImageHashes, exchangeTimestamp, exchangeResponseTimestamp, messageReadTimestamp, exchangeResponseText, exchangeEngineLimitDetail, exchangeSteps, drawnResponseEvents, exchangeStatus, exchangeError, exchangeChangeId, exchangeStarterId, dividerBodyIsSuppressed, opensAwaitingAnswer, hasRenderableResponseContent, isEmptyContinuedExchange, questionDividerResolution, changePanelHasContinuation, findCommandPermissionResolution, findMcpPermissionResolution, findPermissionResolution, findQuestionAnswer, isChangeLifecycleEvent, isLivePartialRow, isLiveReplyRow, isLiveUtteranceRow, isSpeechOnlyTurn, turnBodyFolded, modeToInitiator, originMode, continuationStartedSummary, responseAbortedSummary, eventWaitStoppedSummary, isTurnlessBoundary, isUnsentExchange, agentMessageSender, waitReentryReason, RESPONSE_CANCELED_SUMMARY, triggerRunInterruption } from '../../store/thread-events';
import { LucidosGlyph } from '../shared/LucidosMark';
import { artifacts, appsList, stepsExpanded, detailsExpanded, collapsedExchanges, toggleExchangeCollapsed, expandExchange, collapsedInitiators, toggleInitiatorCollapsed, toggleMessageRoutePanel } from '../../store/store';
import { discardUnsentMessage, editQueuedMessage, removeQueuedMessage, retryUnsentMessage } from '../../store/actions/chat';
import { currentCacheCards } from '../../store/actions/preferences';
import { unsentMessages } from '../../store/unsentMessages';
import { withScrollAnchor } from './CreateThreadView';
import { QuestionBody } from './QuestionCard';
import { isEngineRaisedCard, isTriggerGrantSelection } from '../../store/thread-events';
import { agentExitCopy } from './agentExitCopy';
import { providerBlockCopy, providerCodeLine } from './providerBlockCopy';
import { ProviderBlockActions } from './ProviderBlockActions';
import { teammateName } from '../../store/actions/gatewayUsers';
import { originPopoverHasContent, resolveOrigin } from './messageOrigin';
import { pendingAnswers, unsentPicks } from '../../store/pendingDecisions';
import { CommandPermissionBody, EnginePermissionBody, McpPermissionBody, PermissionBody, engineResolutionNote, pendingVerdicts } from './PermissionCard';
import { ChildCompletionRow, ChildMovedOutRow, ChildStoppedRow } from './ChildCompletionRow';
import { KeepAliveRow } from './CacheKeepAlive';
import { drawsResponseRow, liveStepInBody, responseBody, type BodyRow } from '../../store/event-rendering';
import { Disclosure } from '../shared/Disclosure';
import { HELD_CALLBACK_NOTE, QUEUED_BEHIND_CARD_LABEL, statusLabel as getStatusLabel, isActive as isStatusActive, isTerminated } from '../../store/exchange-status';
import type { ExchangeStatus } from '../../store/exchange-status';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { markdownEngine, renderMarkdown } from '../../utils/renderMarkdown';
import { linkifyPaths } from '../../utils/linkifyPaths';
import { handleMarkdownLinkClick } from '../shared/markdownLinkClick';
import { FormRequestRow } from './FormRequestRow';
import { NO_SIDE_QUESTIONS, SideQuestionGroup, placeInBody } from './SideQuestionCard';
import type { SideQuestion } from '../../store/sideQuestions';
import { ChangeEventRow, CheckpointCard, ContinueButton, RestartRow, FreshSessionRow, HandoffSessionRow, EventDeliveryBody, EventWaitRow, FileList, HeldMessageRow, GeneratedImage, InitiatorPanel, InlineStep, LivePartialBody, SubAgentStepGroup, LiveUtteranceBody, MarkdownBlock, ResponsePanel, ResumeCard, SpokenChip, SpokenReply, TriggerFiredBody, TriggerInterruptionNotice, UserMessageBody, changeAccent, describeExecutor, boundaryCard, turnControls } from './chat-exchange-parts';
import type { BoundaryTurn } from './chat-exchange-parts';
import { EditIcon, TrashIcon, PowerIcon, UserIcon, ApiPlugIcon, TriggerFiredIcon, WarningIcon, ContinuedIcon } from '../shared/icons';
import { useOnScreenInTranscript } from '../../hooks/useOnScreenInTranscript';
import { engineReasonHeadline } from '../../utils/engineEventExplainers';

const NO_ARTIFACTS: string[] = [];
const NO_APPS: App[] = [];

interface Props {
  exchange: Exchange;
  revision: number;
  streamingBuffer: string;
  isLast: boolean;
  threadId: string;
  hasPriorActive?: boolean;
  priorModel?: string;
  priorEffort?: string;
  isContinuableAbort?: boolean;
  isQueued?: boolean;
  readMarker?: ReadMarker;
  threadIsCC: boolean;
  sessionCodingAgent: CodingAgent;
  threadIdle: boolean;
  threadAwaitingAnswer: boolean;
  behindOpenQuestion?: boolean;
  threadCanceling: boolean;
  rowsHidden?: number;
  proposedChangeDesc?: string;
  proposedChangeFileCount?: number;
  proposedChangeSummary?: string;
  matchedEventType?: string;
  matchedEventId?: string;
  matchedPayloadJson?: string;
  pausedBy?: StoredEvent;
  sideQuestions?: readonly SideQuestion[];
}

export function liveRowDrawsNoPanel(userEvent: StoredEvent): boolean {
  if (isLiveReplyRow(userEvent)) return true;
  if (!isLiveUtteranceRow(userEvent)) return false;
  return isLivePartialRow(userEvent) || !(userEvent as { text?: string }).text;
}

function unsentMessageOf(exchange: Exchange): string | undefined {
  if (!isUnsentExchange(exchange)) return undefined;
  const eventId = exchangeStarterId(exchange);
  return eventId && unsentMessages.value.has(eventId) ? eventId : undefined;
}

function heldOnThePress(fn: () => void): (e: MouseEvent) => void {
  return (e) => withScrollAnchor(e.currentTarget as HTMLElement | null, fn);
}

const NO_EVENTS: readonly ResponseEvent[] = [];

function ChatExchangeImpl({ exchange, streamingBuffer, isLast, isQueued, readMarker, threadId, hasPriorActive, priorModel, priorEffort, isContinuableAbort, threadIsCC, sessionCodingAgent, threadIdle, threadAwaitingAnswer, behindOpenQuestion, threadCanceling, rowsHidden = 0, proposedChangeDesc, proposedChangeFileCount, proposedChangeSummary, matchedEventType, matchedEventId, matchedPayloadJson, pausedBy, sideQuestions = NO_SIDE_QUESTIONS, cacheCards }: ImplProps) {
  const showDetails = detailsExpanded.value;
  const showSteps = stepsExpanded.value;
  const artifactPaths = loadedOr(artifacts.value, NO_ARTIFACTS);
  const apps = loadedOr(appsList.value, NO_APPS);

  const userMessage = exchangeUserMessage(exchange);
  const starterId = exchangeStarterId(exchange);
  const userImageHashes = exchangeUserImageHashes(exchange);
  const timestamp = exchangeTimestamp(exchange);
  const readAt = readMarker === 'read' ? messageReadTimestamp(exchange) : undefined;
  const responseTextRaw = exchangeResponseText(exchange);
  const steps = exchangeSteps(exchange, isLast, threadIdle);
  const allEvents = drawnResponseEvents(exchange, isLast, threadIdle, cacheCards);
  const keepAliveRuns = opensAwaitingAnswer(exchange) ? allEvents.filter((e) => e.type === 'cache_keep_alive') : NO_EVENTS;
  const events = keepAliveRuns.length > 0 ? allEvents.filter((e) => e.type !== 'cache_keep_alive') : allEvents;
  const status = exchangeStatus(exchange, streamingBuffer, isLast, hasPriorActive, threadIsCC, threadIdle, threadAwaitingAnswer, behindOpenQuestion);
  const error = exchangeError(exchange);
  const interruption = triggerRunInterruption(exchange);
  const agentExitCard = error?.agentExit && agentExitCopy(error.agentExit, sessionCodingAgent);
  const providerBlockCard = error?.providerError && providerBlockCopy(error.providerError);
  const providerCode = error?.providerError && providerCodeLine(error.providerError);
  const unsentEventId = unsentMessageOf(exchange);
  const unsentText = unsentEventId ? unsentMessages.value.get(unsentEventId)?.body.message.trim() : undefined;

  const engineLimitDetail = !streamingBuffer ? exchangeEngineLimitDetail(exchange) : '';
  const isEngineLimit = !!engineLimitDetail;
  const streamingHtml = streamingBuffer ? renderMarkdown(streamingBuffer, { cache: false }) : '';
  const responseHtml = responseTextRaw ? renderMarkdown(responseTextRaw) : '';
  const responseHtmlCombined = streamingHtml || responseHtml;
  const hasResponse = !!responseHtmlCombined || isEngineLimit;

  const markdown = markdownEngine.value;
  const userMessageHtml = useMemo(
    () => linkifyPaths(renderMarkdown(userMessage), artifactPaths, apps),
    [userMessage, artifactPaths, apps, markdown],
  );

  const hasEvents = events.length > 0;
  const hasSteps = steps.length > 0 || events.some(e => e.type === 'step');

  const isResume = isResumeTurn(exchange);
  const canCollapse = hasResponse || events.some((e) => drawsResponseRow(e, showSteps)) || isResume;
  const isCollapsed = canCollapse && collapsedExchanges.value.has(`${threadId}:${exchange.userSeq}`);
  const bodyFolded = canCollapse && turnBodyFolded(collapsedExchanges.value, threadId, exchange);

  const handleLinkClick = (e: MouseEvent) => handleMarkdownLinkClick(e, apps);

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

  const toggleCollapsed = heldOnThePress(() => toggleExchangeCollapsed(threadId, exchange.userSeq));
  const toggleInitiator = heldOnThePress(() => toggleInitiatorCollapsed(threadId, exchange.userSeq));

  const turnControlsSlot = turnControls({
    detailsOn: showDetails,
    stepsOn: showSteps,
    collapsed: isCollapsed,
    collapsible: canCollapse,
    onToggleDetails: toggleDetails,
    onToggleSteps: toggleSteps,
    onToggleCollapsed: toggleCollapsed,
  });

  const sections = useMemo(
    () => (hasEvents && !bodyFolded ? responseBody(events, { showSteps, showDetails, rowsHidden }) : []),
    [hasEvents, bodyFolded, events, showSteps, showDetails, rowsHidden],
  );
  const bodyPieces = useMemo(
    () => placeInBody(sections, canCollapse && !bodyFolded ? sideQuestions : NO_SIDE_QUESTIONS),
    [sections, canCollapse, bodyFolded, sideQuestions],
  );


  const [liveStepRow, setLiveStepRow] = useState<HTMLElement | null>(null);
  const liveRowIndex = isCollapsed ? -1 : liveStepInBody(sections);
  const markedRow = liveRowIndex >= 0 ? liveStepRow : null;
  const rowOnScreen = useOnScreenInTranscript(markedRow);
  const liveStepOnScreen = markedRow !== null && rowOnScreen;

  const chunkHtmls = useMemo(() => {
    const map = new Map<ResponseEvent, string>();
    for (const piece of bodyPieces) {
      if (piece.kind !== 'section') continue;
      for (const row of piece.rows) {
        if (row.kind === 'text' && row.open) {
          map.set(row.event, linkifyPaths(renderMarkdown(row.event.md), artifactPaths, apps));
        }
      }
    }
    return map;
  }, [bodyPieces, artifactPaths, apps, markdown]);

  const responseHtmlLinkified = useMemo(
    () => linkifyPaths(responseHtmlCombined, artifactPaths, apps, { cache: !streamingBuffer }),
    [responseHtmlCombined, artifactPaths, apps, streamingBuffer],
  );

  const engineCard = exchange.userEvent.type === 'UserQuestionAsked' && isEngineRaisedCard(exchange.userEvent);
  const responseTerminated = exchange.cardWaiting === true
    || (!engineCard && (isTerminated(status) || exchange.questionOvertaken === true));

  const boundaryTurn = boundaryTurnOf(status);
  const heldNote = status === 'held' ? HELD_CALLBACK_NOTE : undefined;
  const initiator = useMemo(
    () => withoutActorHeader(
      describeInitiator(exchange, userMessageHtml, userImageHashes, threadId, responseTerminated, threadIsCC, sessionCodingAgent, { description: proposedChangeDesc, fileCount: proposedChangeFileCount, summary: proposedChangeSummary }, { eventType: matchedEventType, eventId: matchedEventId, payloadJson: matchedPayloadJson }, heldNote),
      exchange.userEvent,
      {
        turn: boundaryTurn,
        actions: exchange.userEvent.type === 'ResponseAborted' && isContinuableAbort ? <ContinueButton threadId={threadId} /> : undefined,
      },
    ),
    [exchange, exchange.userEvent, userMessageHtml, userImageHashes, threadId, responseTerminated, threadIsCC, sessionCodingAgent, proposedChangeDesc, proposedChangeFileCount, proposedChangeSummary, matchedEventType, matchedEventId, matchedPayloadJson, heldNote, boundaryTurn, isContinuableAbort],
  );
  const isChangePanel = isChangeLifecycleEvent(exchange.userEvent);
  const isUserMessageBubble = isUserBubbleEvent(exchange.userEvent) && initiator.variant === 'user';
  const isSpeechOnly = isSpeechOnlyTurn(exchange);
  const senderOrigin = isUserMessageBubble ? resolveOrigin(exchange) : undefined;
  const teammate = senderOrigin?.kind === 'device' ? teammateName(senderOrigin.device_id) : null;
  const isChromeless = isUserMessageBubble || isChangePanel || isSpeechOnly || initiator.chromeless === true;
  const canCollapseInitiator = !isChromeless
    && (!!initiator.summary || !!initiator.details);
  const isInitiatorCollapsed = canCollapseInitiator
    && collapsedInitiators.value.has(`${threadId}:${exchange.userSeq}`);
  const changeId = exchangeChangeId(exchange, threadIsCC);
  const isQueuedUserMessage = !!isQueued && isUserMessageBubble;
  const panelHeld = isQueuedUserMessage && threadAwaitingAnswer ? {}
    : heldNote && !initiator.heldNoteInCard ? { note: heldNote } : undefined;
  const canTakeBack = !threadIsCC || sessionCodingAgent === 'claude-code' || exchange.userEvent.queued_for_permission;
  const queuedMessageId = isQueuedUserMessage && canTakeBack ? exchange.userEvent._eventId : undefined;
  const isLiveRow = liveRowDrawsNoPanel(exchange.userEvent);
  const queuedStatus = (
    <span class="exchange-status-label exchange-status-queued">
      {exchange.userEvent.type === 'MessageReceived' && exchange.userEvent.queued_for_permission
        ? QUEUED_BEHIND_CARD_LABEL
        : 'Queued'}
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
  const isChangeContinuation = isChangePanel && changePanelHasContinuation(exchange);
  const isTerminatedContinuation = (isAbortPanel || isCancelPanel)
    && (hasResponse || hasRenderableResponseContent(events));
  const isTurnlessPanel = isTurnlessBoundary(exchange.userEvent);
  const speechOnlyHasWords = !isSpeechOnly || canCollapse;
  const showResponsePanel = status !== 'held' && (isResume || (!isChangePanel || isChangeContinuation) && (!isAbortPanel || isTerminatedContinuation) && (!isCancelPanel || isTerminatedContinuation) && !isTurnlessPanel && !isUnansweredDivider && !isEmptyContinued && !isQueuedUserMessage && !isLiveRow && speechOnlyHasWords && (hasResponse || hasEvents || showStatus));
  const bodyTakesCards = showResponsePanel && canCollapse && !bodyFolded;
  const executor = describeExecutor(threadIsCC, sessionCodingAgent);

  function renderRow(row: BodyRow) {
    switch (row.kind) {
      case 'text':
        return (
          <Disclosure key={`t${row.key}`} open={row.open}>
            <ReplyHtml
              class="response-chunk"
              textSeq={row.event.seq}
              html={chunkHtmls.get(row.event) ?? ''}
              final
              threadId={threadId}
              eventId={starterId}
            />
          </Disclosure>
        );
      case 'steps':
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
        <InlineWidget
          key={`wg${k}`}
          threadId={threadId}
          appId={evt.app_id}
          params={evt.params}
          label={evt.label}
          eventId={evt.event_id}
          stamped
        />
      );
    }
    if (evt.type === 'spoken_reply') return <SpokenReply key={`sr${k}`} event={evt} />;
    if (evt.type === 'restart') return <RestartRow key={`rs${k}`} event={evt} />;
    if (evt.type === 'fresh_session') return <FreshSessionRow key={`fs${k}`} event={evt} />;
    if (evt.type === 'handoff_session') return <HandoffSessionRow key={`hs${k}`} event={evt} />;
    if (evt.type === 'event_wait') return <EventWaitRow key={`ew${k}`} event={evt} />;
    if (evt.type === 'held_message') return <HeldMessageRow key={`hm${k}`} event={evt} />;
    if (evt.type === 'cache_keep_alive') return <KeepAliveRow key={`ka${k}`} threadId={threadId} status={evt.status} />;
    if (evt.type === 'form_request') return <FormRequestRow key={`fr${k}`} row={evt} threadId={threadId} />;
    if (evt.type === 'empty') return <div key={`e${k}`} class="response-empty-note">{'The agent finished without writing a reply. Ask it to sum up what it did.'}</div>;
    return null;
  }

  const collapseKind = isSpeechOnly ? undefined
    : canCollapse ? 'response'
      : canCollapseInitiator ? 'initiator' : undefined;

  return (
    <div class="chat-exchange" data-event-id={starterId} data-change-id={changeId || undefined}
         data-thread-id={threadId} data-user-seq={exchange.userSeq} data-collapse-kind={collapseKind}
         data-head-clamped={rowsHidden > 0 ? '' : undefined}>
      {drawsInitiatorPanel(exchange) && (
      <InitiatorPanel
        key="initiator"
        initiator={initiatorStatus
          ? { ...initiator, status: initiatorStatus }
          : initiator}
        timestamp={formatMessageTimestamp(readAt ?? timestamp)}
        onActorClick={initiator.actorClickable === false || !originPopoverHasContent(exchange)
          ? undefined
          : (e) => openInfoPanel('origin', e)}
        onBodyClick={handleLinkClick}
        bubble={isUserMessageBubble}
        teammate={teammate}
        chromeless={isChromeless}
        collapsible={canCollapseInitiator}
        collapsed={isInitiatorCollapsed}
        onToggle={canCollapseInitiator ? toggleInitiator : undefined}
        held={panelHeld}
      />
      )}

      <Disclosure open={!!interruption} key="trigger-interruption" class="initiator-panel" bodyClass="initiator-body">
        {interruption && <TriggerInterruptionNotice event={interruption} />}
      </Disclosure>

      {keepAliveRuns.map((run, i) => run.type === 'cache_keep_alive' && (
        <div class="keep-alive-under-card" key={`ka${i}`}>
          <KeepAliveRow threadId={threadId} status={run.status} />
        </div>
      ))}

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
          {[
            ...(isResume ? [<div class="response-content" key="resume"><ResumeCard exchange={exchange} /></div>] : []),
            ...(hasEvents ? [] : [
              <div class="response-content markdown-content" key="streamed" onClick={handleLinkClick}>
                <ReplyHtml html={responseHtmlLinkified} final={!streamingBuffer} threadId={threadId} eventId={starterId} />
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
              <div class="authored-html" dangerouslySetInnerHTML={{ __html: linkifyPaths(renderMarkdown(engineLimitDetail), artifactPaths, apps) }} />
            </div>
          )}
        </ResponsePanel>
      )}

      {!bodyTakesCards && sideQuestions.length > 0 && (
        <SideQuestionGroup key="side-questions:after:turn" items={sideQuestions} />
      )}

      {error && (
        <div class="exchange-error" data-event-id={error.eventId || undefined}>
          <strong>{unsentEventId ? 'Not sent' : error.spendCap ? 'Spend cap reached' : providerBlockCard?.heading ?? agentExitCard?.heading ?? 'The reply failed'}</strong>
          {unsentText && <blockquote class="exchange-error-quote">{unsentText}</blockquote>}
          {unsentEventId
            ? (
              <>
                <p>{error.message}</p>
                <div class="exchange-error-actions">
                  <button type="button" class="action-btn action-btn-secondary exchange-error-discard" onClick={() => discardUnsentMessage(unsentEventId)}>
                    Discard
                  </button>
                  <button type="button" class="action-btn exchange-error-retry" onClick={() => void retryUnsentMessage(unsentEventId)}>
                    Retry
                  </button>
                </div>
              </>
            )
            : error.spendCap
              ? <><p>{providerLabel(error.spendCap.provider)}. {spendCapResetLabel(error.spendCap.resets_at)} You can try again after raising the cap.</p><blockquote class="exchange-error-quote">{error.spendCap.message}</blockquote>{agentExitCard && <p class="exchange-error-reason">{agentExitCard.reason}</p>}{agentExitCard && isLast && <div class="exchange-error-actions"><ContinueButton threadId={threadId} label="Try again" /></div>}</>
            : providerBlockCard
              ? (
                <>
                  <p class="exchange-error-output">{error.message}</p>
                  {error.providerError?.detail && <blockquote class="exchange-error-quote">{error.providerError.detail}</blockquote>}
                  <p class="exchange-error-reason">{providerBlockCard.reason}</p>
                  {isLast && <ProviderBlockActions threadId={threadId} sessionCodingAgent={sessionCodingAgent} />}
                </>
              )
            : agentExitCard
              ? (
                <>
                  <p class="exchange-error-output">{error.message}</p>
                  <p class="exchange-error-reason">{agentExitCard.reason}</p>
                  {isLast && (
                    <div class="exchange-error-actions">
                      <ContinueButton threadId={threadId} label="Try again" />
                    </div>
                  )}
                </>
              )
              : (
                <>
                  <p>{error.message}</p>
                  {providerCode && <p class="exchange-error-reason">{providerCode}</p>}
                  <p>Send a message to try again.</p>
                </>
              )}
        </div>
      )}
    </div>
  );
}

export const TOOL_CALL_CAP_OPENING = 'Per-turn limit';

function engineLimitHeading(detail: string): string {
  return detail.startsWith(TOOL_CALL_CAP_OPENING) ? 'Tool-call limit reached' : 'Lucidos ended this turn';
}

function sameMarks<T>(a: Set<T> | undefined, b: Set<T> | undefined): boolean {
  if (a === b) return true;
  if (!a || !b) return (a?.size ?? 0) === (b?.size ?? 0);
  if (a.size !== b.size) return false;
  for (const mark of a) if (!b.has(mark)) return false;
  return true;
}

function sameSideQuestions(a: readonly SideQuestion[] = NO_SIDE_QUESTIONS, b: readonly SideQuestion[] = NO_SIDE_QUESTIONS): boolean {
  return a.length === b.length && a.every((card, i) => card === b[i]);
}

function userBubbleText(exchange: Exchange): string | undefined {
  const ev = exchange.userEvent;
  if (ev.type === 'MessageReceived') return ev.text;
  return ev.type === 'SpokenReplyGenerated' ? ev.text : undefined;
}

function unsentEventIdOf(exchange: Exchange): string | undefined {
  return exchange.typedAnswer?.state === 'unsent' ? exchange.typedAnswer.unsentEventId : undefined;
}

export function chatExchangePropsEqual(prev: ImplProps, next: ImplProps): boolean {
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
  if (prev.sessionCodingAgent !== next.sessionCodingAgent) return false;
  if (prev.threadIdle !== next.threadIdle) return false;
  if (prev.threadAwaitingAnswer !== next.threadAwaitingAnswer) return false;
  if (prev.behindOpenQuestion !== next.behindOpenQuestion) return false;
  if (prev.threadCanceling !== next.threadCanceling) return false;
  if (prev.rowsHidden !== next.rowsHidden) return false;
  if (prev.proposedChangeDesc !== next.proposedChangeDesc) return false;
  if (prev.proposedChangeFileCount !== next.proposedChangeFileCount) return false;
  if (prev.proposedChangeSummary !== next.proposedChangeSummary) return false;
  if (prev.matchedEventType !== next.matchedEventType) return false;
  if (prev.matchedEventId !== next.matchedEventId) return false;
  if (prev.matchedPayloadJson !== next.matchedPayloadJson) return false;
  if (prev.pausedBy !== next.pausedBy) return false;
  if (!sameSideQuestions(prev.sideQuestions, next.sideQuestions)) return false;
  if (prev.cacheCards !== next.cacheCards) return false;
  const a = prev.exchange;
  const b = next.exchange;
  if (a.userSeq !== b.userSeq) return false;
  if (userBubbleText(a) !== userBubbleText(b)) return false;
  if (a.liveReplyText !== b.liveReplyText) return false;
  if (a.questionOvertaken !== b.questionOvertaken) return false;
  if (a.cardWaiting !== b.cardWaiting) return false;
  if (a.continuationMoved !== b.continuationMoved) return false;
  if (a.releasedFromHold !== b.releasedFromHold) return false;
  if (a.typedAnswer?.text !== b.typedAnswer?.text) return false;
  if (a.typedAnswer?.state !== b.typedAnswer?.state) return false;
  if (unsentEventIdOf(a) !== unsentEventIdOf(b)) return false;
  if (a.steps.length !== b.steps.length) return false;
  const aLast = a.steps[a.steps.length - 1]?.seq;
  const bLast = b.steps[b.steps.length - 1]?.seq;
  if (aLast !== bLast) return false;
  if (!sameMarks(a.blockedStepSeqs, b.blockedStepSeqs)) return false;
  if (!sameMarks(a.deniedStepSeqs, b.deniedStepSeqs)) return false;
  if (!sameMarks(a.deliveredHeldIds, b.deliveredHeldIds)) return false;
  return true;
}

type ImplProps = Props & { cacheCards: boolean };

const MemoChatExchange = memo(ChatExchangeImpl, chatExchangePropsEqual);

export function ChatExchange(props: Props) {
  const cacheCards = currentCacheCards();
  if (!cacheCards && props.exchange.userEvent.type === 'CacheKeepAliveStatusChanged') return null;
  return <MemoChatExchange {...props} cacheCards={cacheCards} />;
}

export function shouldShowResponseStatusBadge(
  userEvent: ThreadEvent,
  statusClass: string,
): boolean {
  if (userEvent.type === 'UserQuestionAsked' && statusClass === 'canceled') return false;
  return !abortPromisesAutoResume(userEvent);
}


type InitiatorVariant = 'user' | 'system' | 'trigger' | 'lucidos';

export interface ProposedChangeSeed {
  description?: string;
  fileCount?: number;
  summary?: string;
}

export interface InitiatorDescriptor {
  variant: InitiatorVariant;
  icon: ComponentChildren;
  label: string;
  status?: ComponentChildren;
  summary?: string;
  details?: ComponentChildren;
  accent?: string;
  actorClickable?: boolean;
  chromeless?: boolean;
  heldNoteInCard?: boolean;
}

function initiatorSummary(exchange: Exchange): string {
  const ev = exchange.userEvent;
  switch (ev.type) {
    case 'TriggerStarted':           return '';
    case 'ContinuationStarted':         return continuationStartedSummary(ev.reason, ev.actor);
    case 'ResponseAborted':            return responseAbortedSummary(ev.actor, ev.cause);
    case 'MissingHardeningDetected': return 'Hardening needed';
    case 'MergeConflictDetected':    return 'Merge conflict';
    case 'CodingAgentPromptSent':    return 'Instructions from Lucidos';
    case 'ChangeApplied':
    case 'ChangeDiscarded':
    case 'ChangeReverted':
    case 'ChangeApplyFailed':        return '';
    case 'PromptInjected':           return injectedMessageSummary(ev);
    case 'EventWaitCanceled':        return eventWaitStoppedSummary(ev.reason);
    case 'MessageReceived': {
      if (ev.origin?.kind === 'api') return 'API message';
      if (ev.origin?.kind === 'unattested_caller') return ev.origin.claimed_workspace
        ? `Message from workspace "${ev.origin.claimed_workspace}", by its own account`
        : `Message from ${unattestedCallerLabel(ev.origin, ev.origin.claimed_device_id ? turnDeviceName(ev.origin.claimed_device_id, { knownOnly: true }) : undefined).replace(/^A /, 'a ').replace(/^An /, 'an ')}`;
      if (modeToInitiator(ev.mode) !== 'system') return '';
      const headline = ev.origin?.kind === 'engine' ? engineReasonHeadline(ev.origin.reason) : null;
      if (headline) return `${headline.label}: ${headline.value}`;
      const sender = agentMessageSender(ev.origin);
      const said = sender ? `Message from ${sender}` : 'Message from an agent';
      return exchange.releasedFromHold ? `${said}, held until you replied` : said;
    }
    case 'UserQuestionAsked':            return '';
    case 'CodingAgentPermissionRequest': return '';
    case 'McpConsentRequested':          return `Tool consent requested: ${ev.tool}`;
    case 'ChildThreadCompleted':         return '';
    case 'ChildThreadStopped':           return '';
    case 'ChildReportFiled':             return '';
    case 'ChildThreadDetached':          return '';
    default:                         return '';
  }
}

function injectedMessageSummary(ev: Extract<StoredEvent, { type: 'PromptInjected' }>): string {
  const reentry = waitReentryReason(ev);
  if (reentry) return reentry.outcome === 'expired' ? 'Wait timed out' : 'Event arrived';
  if (ev.origin?.kind === 'device') return 'Your message, read while the agent worked';
  const sender = agentMessageSender(ev.origin);
  return sender ? `Message from ${sender}` : 'Message from Lucidos';
}

function actorVariant(actor: Parameters<typeof actorInitiator>[0]): InitiatorVariant {
  return originMode(actor) === 'agent' ? 'lucidos' : 'system';
}

export function actorInitiator(actor: MessageOrigin | undefined): { icon: ComponentChildren; label: string } {
  if (actor?.kind === 'system') return { icon: <PowerIcon />, label: SYSTEM_LABEL };
  if (actor?.kind === 'device') return { icon: <UserIcon />, label: 'You' };
  if (actor?.kind === 'unattested_caller') return { icon: null, label: unattestedCallerLabel(actor, actor.claimed_device_id ? turnDeviceName(actor.claimed_device_id, { knownOnly: true }) : undefined) };
  switch (originMode(actor)) {
    case 'human':  return { icon: <ApiPlugIcon />, label: API_CALLER_LABEL };
    case 'agent':  return { icon: <LucidosGlyph />, label: LUCIDOS_AGENT_LABEL };
    case 'engine': return { icon: <LucidosGlyph />, label: ENGINE_LABEL };
  }
}

function youInitiator(rest: Partial<InitiatorDescriptor> = {}): InitiatorDescriptor {
  return { variant: 'user', icon: <UserIcon />, label: 'You', ...rest };
}

function engineInitiator(summary: string, details?: ComponentChildren): InitiatorDescriptor {
  return { variant: 'system', icon: <LucidosGlyph />, label: ENGINE_LABEL, summary, details };
}

function withoutActorHeader(
  d: InitiatorDescriptor,
  starter: StoredEvent,
  card: { turn: BoundaryTurn; actions?: ComponentChildren },
): InitiatorDescriptor {
  const narrated = d.label === ENGINE_LABEL || d.label === SYSTEM_LABEL || d.icon === null;
  if (!narrated) return d;
  const summary = d.summary ?? (d.icon === null ? d.label : undefined);
  if (!summary) return { ...d, chromeless: true };
  return { ...d, chromeless: true, summary: undefined, details: boundaryCard(starter, summary, { carried: d.details, ...card }) };
}

function boundaryTurnOf(status: ExchangeStatus): BoundaryTurn {
  if (isStatusActive(status) || status === 'awaiting-answer') return 'working';
  if (status === 'done') return 'done';
  if (status === 'queued' || status === 'held') return 'waiting';
  return 'stopped';
}

function actionInitiator(label: string, details?: ComponentChildren): InitiatorDescriptor {
  return { variant: 'system', icon: null, label, details };
}

type DividerTerminalKind = 'canceled' | 'superseded' | 'dropped' | 'waiting';

function dividerStatus(
  resolved: boolean,
  resolvedLabel: string,
  droppedLabel: string,
  terminal: DividerTerminalKind | null,
  pick: PendingPick,
): ComponentChildren {
  if (resolved) return <span class="exchange-status-label exchange-status-done">{resolvedLabel}</span>;
  if (terminal === 'canceled') return <span class="exchange-status-label exchange-status-canceled">{'Canceled'}<span class="exchange-status-x exchange-status-glyph">{'✕'}</span></span>;
  if (terminal === 'superseded') return <span class="exchange-status-label exchange-status-dropped">{'Superseded'}</span>;
  if (terminal === 'dropped') return <span class="exchange-status-label exchange-status-dropped">{droppedLabel}</span>;
  if (terminal === 'waiting') return <span class="exchange-status-label exchange-status-dropped">{'Waits for the earlier card'}</span>;
  return <AwaitingStatus {...pick} />;
}

type PendingPick = {
  picks: ReadonlySignal<ReadonlyMap<string, unknown>>;
  id: string;
  typed?: TypedAnswer['state'];
  unsent?: ReadonlySignal<ReadonlyMap<string, unknown>>;
};

export function AwaitingStatus({ picks, id, typed, unsent }: PendingPick) {
  if (picks.value.has(id) || typed === 'sending') {
    return <span class="exchange-status-label exchange-status-sending">{'Sending'}</span>;
  }
  if (typed === 'unsent' || unsent?.value.has(id)) return <span class="exchange-status-label exchange-status-not-sent">{'Not sent'}</span>;
  return <span class="exchange-status-label exchange-status-awaiting">{'Needs your answer'}</span>;
}

type PermissionVerdict = Pick<ResolvedPermission, 'allowed' | 'reason' | 'persist_scope' | 'standing_deny'>;

function permissionVerdict(step: PermissionVerdict | undefined): PermissionVerdict | undefined {
  return step && {
    allowed: step.allowed,
    reason: step.reason,
    persist_scope: step.persist_scope,
    standing_deny: step.standing_deny,
  };
}

function permissionResolvedLabel(verdict: PermissionVerdict | undefined): string {
  if (!verdict || !engineResolutionNote(verdict)) return 'Resolved';
  return verdict.allowed ? 'Allowed by Lucidos' : 'Closed';
}

export function describeInitiator(
  exchange: Exchange,
  userMessageHtml: string,
  userImageHashes: string[],
  threadId: string,
  responseTerminated: boolean = false,
  threadIsCC: boolean = false,
  sessionCodingAgent: CodingAgent = 'claude-code',
  proposedChange: ProposedChangeSeed = {},
  matched?: { eventType?: string; eventId?: string; payloadJson?: string },
  heldNote?: string,
): InitiatorDescriptor {
  const ev = exchange.userEvent;
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
  if (exchange.continuationFragment) return { variant: 'system', icon: null, label: '' };
  const summary = initiatorSummary(exchange);
  switch (ev.type) {
    case 'TriggerStarted':
      return {
        variant: 'trigger',
        icon: <TriggerFiredIcon />,
        label: ENGINE_LABEL,
        details: <TriggerFiredBody event={ev} />,
      };
    case 'ContinuationStarted':
      if (ev.actor?.kind === 'device') return actionInitiator(summary);
      return { variant: actorVariant(ev.actor), ...actorInitiator(ev.actor), summary };
    case 'ResponseAborted':
      if (ev.actor?.kind === 'device') {
        return actionInitiator(summary);
      }
      return {
        variant: actorVariant(ev.actor),
        ...actorInitiator(ev.actor),
        summary,
      };
    case 'ResponseCanceled':
      return actionInitiator(RESPONSE_CANCELED_SUMMARY);
    case 'EventWaitCanceled':
      return actionInitiator(summary);
    case 'MissingHardeningDetected':
      return engineInitiator(summary);
    case 'MergeConflictDetected':
      return engineInitiator(
        summary,
        (ev.files?.length ?? 0) > 0 ? <FileList files={ev.files!} /> : undefined,
      );
    case 'CodingAgentPromptSent':
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
      if (ev.origin?.kind === 'device') {
        return actionInitiator(summary, <MarkdownBlock html={userMessageHtml} />);
      }
      return {
        variant: actorVariant(ev.origin),
        ...actorInitiator(ev.origin),
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
      if (
        ev.origin?.kind === 'api'
        || ev.origin?.kind === 'unattested_caller'
        || modeToInitiator(ev.mode) === 'system'
      ) {
        return { variant: actorVariant(ev.origin), summary, details, ...actorInitiator(ev.origin) };
      }
      if (ev.voice_session_id) {
        return youInitiator({ details, status: <SpokenChip /> });
      }
      return youInitiator({ details });
    }
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
      return youInitiator({
        details: userMessageHtml
          ? <UserMessageBody html={userMessageHtml} imageHashes={[]} />
          : undefined,
        status: <SpokenChip />,
      });
    case 'ChildThreadCompleted':
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
    case 'ChildReportFiled':
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
            subThreadPendingChanges={ev.sub_thread_pending_changes}
            filed
          />
        ),
      };
    case 'CacheKeepAliveStatusChanged':
      return {
        variant: 'system',
        icon: <LucidosGlyph />,
        label: ENGINE_LABEL,
        actorClickable: false,
        details: <KeepAliveRow threadId={threadId} status={ev.status} />,
      };
    case 'ChildThreadStopped':
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
      const answered = findQuestionAnswer(exchange, ev.tool_use_id);
      const unanswered = questionDividerResolution(exchange);
      const agent = describeExecutor(threadIsCC, sessionCodingAgent);
      return {
        variant: 'lucidos',
        icon: agent.icon,
        label: agent.label,
        status: dividerStatus(
          !!answered && !unanswered,
          'Answered',
          'Unanswered',
          unanswered ?? (exchange.cardWaiting ? 'waiting' : responseTerminated ? 'dropped' : null),
          { picks: pendingAnswers.map, id: ev.tool_use_id, typed: exchange.typedAnswer?.state, unsent: unsentPicks.map },
        ),
        details: ev.permission_request && !ev.multi_select ? (
          <EnginePermissionBody
            threadId={threadId}
            eventId={exchangeStarterId(exchange)}
            toolUseId={ev.tool_use_id}
            question={ev.question}
            options={ev.options ?? []}
            request={ev.permission_request}
            resolved={answered?.answer}
            typedAnswer={exchange.typedAnswer}
            terminated={responseTerminated}
          />
        ) : (
          <QuestionBody
            threadId={threadId}
            eventId={exchangeStarterId(exchange)}
            toolUseId={ev.tool_use_id}
            question={ev.question}
            options={ev.options ?? []}
            multiSelect={ev.multi_select}
            grantSelection={isTriggerGrantSelection(ev)}
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
      const agent = describeExecutor(true, sessionCodingAgent);
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
              grant_offer: ev.grant_offer,
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
              grant_head: ev.grant_head,
              grant_offer: ev.grant_offer,
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
    case 'UserOnlyMessagePosted':
      return youInitiator({
        details: userMessageHtml || userImageHashes.length > 0
          ? <UserMessageBody html={userMessageHtml} imageHashes={userImageHashes} />
          : undefined,
      });
    case 'McpConsentRequested':
      return actionInitiator(summary);
    default:
      return youInitiator();
  }
}

export { describeExecutor } from './chat-exchange-parts';
