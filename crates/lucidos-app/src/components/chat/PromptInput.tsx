import { Fragment, type VNode } from 'preact';
import { useRef, useEffect, useLayoutEffect, useState, useMemo } from 'preact/hooks';
import { Overlay } from '../shared/Overlay';
import { Disclosure } from '../shared/Disclosure';
import { signal, untracked, useSignalEffect } from '@preact/signals';
import { pendingChatMessage, showToast, openImagePopupFromGroup, focusedThreadId, threadMap, panelUrl, panelTitle, cancelingThreadIds, answeringThreadIds, clearThreadAnswering, effectiveThreadStatus, currentApp, wipPreviewThreadId, promptSendCollapsing, composeViewActive, scaledDurationMs } from '../../store/store';
import { resolveCodingAgent } from '../../store/composeSelections';
import { chatTurnSelection } from '../../store/threadModelSelections';
import { discardUnsentAnswers, sendMessage, handleCancelExchange } from '../../store/actions/chat';
import { currentChatContext, type ChatContext } from '../../store/actions/chatContext';
import { answerThreadQuestion } from '../../store/actions/chat-claude-code';
import { type AnswerKind, type ThreadState } from '../../store/thread-events';
import {
  multiSelectedByToolUse,
  getMultiSelectedIds,
  setMultiSelectedIds,
} from './QuestionCard';
import { pendingAnswers } from '../../store/pendingDecisions';
import { updateCompose, updateComposeSelection, sendCompose, sendFollowup, ensureFocusedComposeThread } from '../../store/actions/compose';
import { focusPane } from '../../store/actions/pane';
import { openAppById } from '../../store/actions/apps';
import { pushNavState } from '../../store/actions/navigation';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { getDraft } from '../../store/composeDrafts';
import { askSideQuestion, routeSideQuestion, sideQuestionModeOn } from '../../store/sideQuestions';
import { ComposeDestinationRow } from './ComposeDestinationRow';
import { followAnsweredQuestion, followCanceledTurn, followSentMessage, followSideQuestion } from './scrollState';
import { CaptureIcon, CheckIcon, ImageIcon, CameraIcon, FileIcon, CloseIcon, ClearIcon, GlobeIcon, SendArrowIcon, StopIcon, WarningIcon } from '../shared/icons';
import { BlobImage } from '../shared/BlobImage';
import { closeControlMenus, codingAgentMenuOpenRequest, rungFoldOpenRequest } from './controlMenuRequests';
import {
  codingAgentMenuAction, composerRowMembers, isComposeContext, LazyCodingAgentControlMenu, PromptActionsRight, promptRowToggles,
  sideQuestionToggleAction,
} from './PromptRowControls';
import { CODING_AGENT_MENU_FOLD_KEY, menuCodingAgent } from './ControlMenuTrigger';
import { LazyRungControl } from './LazyRungControl';
import { RUNG_FOLD_KEY, RungMenuIcon } from './RungButtons';
import { renderHeaderAction, renderMenuAction, type HeaderActionSpec } from '../layout/headerActions';
import { OverflowMenu } from '../shared/OverflowMenu';
import { useComposerCompact } from '../../hooks/useComposerCompact';
import { FOLD_KEY_ATTR, usePromptActionCollapse, type FoldGroup, type PromptMerge } from '../../hooks/usePromptActionCollapse';
import { TodoPanelSlot, closeTodoPanel, todoIndicatorAction } from './todoIndicator';
import { WaitingPanelHost, closeWaitingPanel, waitingIndicatorAction } from './WaitingPanel';
import { composerBannerState, DIFF_APPLY_MERGE, DiffApplyMenu, getBannerActions, getWaitingState, getStandaloneActions } from './WaitingBanner';
import { ActionSlot, SlotFace } from './ActionSlot';
import { composeHasContent, sideQuestionModeActive, computeMorphMode, computeAnswerActionMode, computePromptEscapeAction, dispatchSend, computeSubmitMultiCount, recoverableAnswerDraft, findLatestPendingQuestion, promptPlaceholder, threadHasConversation, shouldClearCanceling, shouldClearSubmitting, submittingThreadIds, queuedUploadSends, queueUploadSend, clearQueuedUploadSend, uploadBlockedSends, markUploadBlockedSend, settleQueuedUploadSends, uploadSendNotice, uploadSendNoticeText, clearSubmittingThread, armCancelSettle, isCancelSettling, promptStopRequested, promptSideQuestionRequested, sideQuestionBlocker, composerEnterSends, type UploadSendIntent } from './prompt-input-helpers';
import { currentComposerEnterKey } from '../../store/actions/preferences';
import { canceledQuestionByThread, setCanceledQuestion, canceledWhileAwaitingByThread, setCanceledWhileAwaiting } from '../../store/canceledQuestions';
import { NotReadyStrip } from './NotReadyStrip';
import { HoldStrip } from './HoldStrip';
import { mentionCandidates, mentionQueryAt, type MentionCandidate } from './mentions';
import { mentionableUsers, mentionsEnabled } from '../../store/actions/users';
import { useAgentPaused } from '../../hooks/useAgentPaused';
import { CODING_AGENT_NAME } from './agentExitCopy';
import { LUCIDOS_AGENT_LABEL } from '../../store/thread-events';
import { lazyComponent } from '../../utils/lazyComponent';
export * from './prompt-input-helpers';
import { composeHandlers } from './composeHandlers';
import { focusIfNeeded } from '../../utils/dom';
import { threadEntryFocusTarget } from './choiceCardNav';
import { focusIntoPane } from '../layout/paneFocus';
import { syncTextareaValue, promptCaretRequest, requestPromptCaret, placeTextareaCaret } from './promptValueSync';
import { endPromptHistory, sentPrompts, stepPromptHistory } from './promptHistory';
import { effectiveCodingAgentBackend, effectiveSendMode } from './promptToggleMode';
import { resizeTextarea, remeasureTextarea, isTextareaHeightAnimating, useFontMetricsResize, useWidthRemeasure, animateTextareaHeightFrom, easeEmptiedTextarea, remeasureTextareaForPlaceholder } from './promptResize';
import { isMobile } from '../../utils/viewport';
import { cameraIsAvailable } from '../../utils/platform';
import { isReducedMotion } from '../../utils/motion';
import { createTapGate } from '../../utils/tapGesture';
import { useFaceShownAt, useTouchActivated } from '../../hooks/useTouchActivated';
import { errorDetail } from '../../utils/errorDetail';
import { extractPasteUrl, escapeMarkdownLinkText } from '../../utils/extractPasteUrl';
import { PROSE_TEXT_ATTRS } from '../../utils/noAutofill';
import { attachDrawnCaret } from '../../utils/drawnCaret';
import { attachedImagesForCurrentThread, getAttachedImages, markHashesAsSent, removeAttachedImage, type AttachedImage } from './pastedImages';
import { getPendingUploads, hasInFlightUploads, uploadsGate, pendingUploads } from '../../store/pendingUploads';
import { cancelPendingUpload, retryPendingUpload } from '../../store/actions/imageUploads';
import { attachImageToActiveDraft } from './attachToDraft';
import { PendingUploadChip } from './PendingUploadChip';
import { computeCaptureGeometry, readDeviceAngle } from './cameraGeometry';
import { isImeComposingKey } from '../../utils/ime';
import { passSendGate, sendGateAsks } from './rungSwitch';

const SuccessorCard = lazyComponent(() => import('./SuccessorCard').then((m) => m.SuccessorCard));
const HandedOffCard = lazyComponent(() => import('./HandedOffCard').then((m) => m.HandedOffCard));
const MentionSuggestions = lazyComponent(() => import('./MentionSuggestions').then((m) => m.MentionSuggestions));

const attachMenuOpen = signal(false);
const cameraOpen = signal(false);
const MORE_TRIGGER_ATTRS = { 'data-row-item': 'fold', [FOLD_KEY_ATTR]: '' };

function closeFoldedPanels(): void {
  attachMenuOpen.value = false;
  closeTodoPanel();
  closeWaitingPanel();
}

function closeRowPanels(): void {
  closeFoldedPanels();
  closeControlMenus();
}
const TOGGLES_FADE_MS = 300;
const TOGGLES_FADE_SLACK_MS = 50;
const UPLOAD_BLOCKS_ANSWER_TOAST = 'An image is still uploading. Submit again once it finishes.';
export const ANSWER_CANCEL_TOOLTIP = 'Cancel this question and ask something else';


export function opensSlashMenu(value: string, claudeCodeMode: boolean): boolean {
  return claudeCodeMode && value === '/';
}

function addImageFile(file: File) {
  attachImageToActiveDraft(file).catch((err) => {
    showToast('Failed to attach image: ' + errorDetail(err), 'error');
  });
}


export const NO_CAMERA_API = 'This page cannot open a camera. The camera needs a secure connection.';

export function openRearCamera(): Promise<MediaStream> {
  if (!cameraIsAvailable()) return Promise.reject(new Error(NO_CAMERA_API));
  return navigator.mediaDevices.getUserMedia({ video: { facingMode: 'environment' } });
}

function CameraCapture() {
  const videoRef = useRef<HTMLVideoElement>(null);
  const streamRef = useRef<MediaStream | null>(null);

  useEffect(() => {
    let canceled = false;
    openRearCamera()
      .then((stream) => {
        if (canceled) { stream.getTracks().forEach((t) => t.stop()); return; }
        streamRef.current = stream;
        if (videoRef.current) {
          videoRef.current.srcObject = stream;
        }
      })
      .catch((err: unknown) => {
        showToast(`Could not access camera: ${errorDetail(err)}`, 'error');
        cameraOpen.value = false;
      });
    return () => {
      canceled = true;
      streamRef.current?.getTracks().forEach((t) => t.stop());
    };
  }, []);

  function capture() {
    const video = videoRef.current;
    if (!video) return;
    const geom = computeCaptureGeometry(video.videoWidth, video.videoHeight, readDeviceAngle());
    const canvas = document.createElement('canvas');
    canvas.width = geom.canvasWidth;
    canvas.height = geom.canvasHeight;
    const ctx = canvas.getContext('2d');
    if (!ctx) {
      showToast('Could not capture photo: the browser refused a drawing surface', 'error');
      close();
      return;
    }
    ctx.translate(geom.translateX, geom.translateY);
    ctx.rotate(geom.rotateRadians);
    ctx.drawImage(video, 0, 0);
    canvas.toBlob((blob) => {
      if (blob) addImageFile(new File([blob], 'camera.jpg', { type: 'image/jpeg' }));
      else showToast('Could not capture photo: the browser produced no image', 'error');
      close();
    }, 'image/jpeg', 0.9);
  }

  function close() {
    streamRef.current?.getTracks().forEach((t) => t.stop());
    cameraOpen.value = false;
  }

  return (
    <Overlay open onClose={close} overlayClass="camera-overlay" panelClass="surface surface-raised camera-container" panelRole="dialog">
      <video ref={videoRef} autoPlay playsInline muted class="camera-video" />
      <div class="camera-controls">
        <button class="camera-capture-btn" onClick={capture} aria-label="Take photo" data-tooltip="Take photo">
          <CaptureIcon />
        </button>
        <button class="action-btn action-btn-secondary" onClick={close}>Cancel</button>
      </div>
    </Overlay>
  );
}

export function CameraCaptureSlot() {
  return cameraOpen.value ? <CameraCapture /> : null;
}

function wipPreviewAction(ft: ThreadState | undefined): HeaderActionSpec | null {
  if (!ft || ft.meta.codingAgentKind !== 'app') return null;
  if (ft.meta.codingAgentChangeState.kind === 'none') return null;
  const folder = ft.meta.codingAgentFolder;
  const appId = folder ? folder.split('/').filter(Boolean).pop() : undefined;
  if (!appId) return null;
  const wipOn = wipPreviewThreadId.value === ft.meta.id;
  return {
    key: 'wip-preview-toggle',
    dataRole: 'wip-preview-toggle',
    label: wipOn ? 'Stop WIP app preview' : 'Show WIP app preview',
    tooltip: wipOn
      ? 'Showing the WIP app preview from this thread’s worktree. Click to return to the live app.'
      : 'Preview the in-flight changes from this app coding-agent thread in the panel.',
    icon: () => (
      <svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
        <path d="M8 3C4.5 3 1.7 5.3 0.5 8c1.2 2.7 4 5 7.5 5s6.3-2.3 7.5-5c-1.2-2.7-4-5-7.5-5zm0 8a3 3 0 1 1 0-6 3 3 0 0 1 0 6zm0-1.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3z"/>
      </svg>
    ),
    active: wipOn,
    onClick: () => {
      if (wipOn) {
        wipPreviewThreadId.value = null;
        pushNavState();
        return;
      }
      void (async () => {
        if (currentApp.value?.id !== appId) {
          await openAppById(appId);
          if (currentApp.value?.id !== appId) return;
        }
        wipPreviewThreadId.value = ft.meta.id;
        pushNavState();
      })();
    },
  };
}


export function PromptInput() {
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const promptActionsAreaRef = useRef<HTMLDivElement>(null);
  const composerCompact = useComposerCompact(promptActionsAreaRef);
  const morphGate = useMemo(() => createTapGate(), []);
  function morphTapPassed(): boolean {
    const moved = morphGate.tapRejection();
    if (moved === null) return true;
    showToast(`Tap ignored: it moved ${moved}px and read as a swipe. Try again.`, 'info');
    return false;
  }
  const morphActivationGate = {
    pass: morphTapPassed,
    spend: morphGate.spend,
    aborted: morphGate.wasAborted,
  };
  useSignalEffect(() => {
    const msg = pendingChatMessage.value;
    if (!msg) return;
    pendingChatMessage.value = null;
    sendMessage(msg, undefined, { context: currentChatContext() }).catch((error) => {
      showToast('Failed to send message: ' + errorDetail(error), 'error');
    });
  });

  const tid = focusedThreadId.value;
  const composeText = getDraft(tid).text;
  const hasText = composeText.length > 0;

  const prevTidRef = useRef<string | null | undefined>(undefined);
  const wasComposeViewRef = useRef(false);
  const caretRequest = promptCaretRequest.value;
  const lastCaretSeqRef = useRef(caretRequest.seq);
  useLayoutEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    const sameThread = prevTidRef.current === tid;
    const isComposeView = composeViewActive.value;
    const caretRequested = caretRequest.seq !== lastCaretSeqRef.current;
    lastCaretSeqRef.current = caretRequest.seq;
    const changed = syncTextareaValue(el, composeText, sameThread && !caretRequested);
    if (caretRequested) placeTextareaCaret(el, caretRequest.side);
    if (changed) {
      const animateSwitch = !sameThread && wasComposeViewRef.current && isComposeView
        && !isMobile() && !isReducedMotion();
      const fromHeight = animateSwitch ? el.style.height : '';
      autoResize();
      if (animateSwitch && fromHeight) {
        animateTextareaHeightFrom(el, fromHeight);
      } else {
        requestAnimationFrame(() => requestAnimationFrame(() => autoResize()));
      }
    }
    if (!sameThread && !isMobile()) {
      requestAnimationFrame(() => focusIntoPane(threadEntryFocusTarget(el)));
    }
    prevTidRef.current = tid;
    wasComposeViewRef.current = isComposeView;
  }, [tid, composeText, caretRequest]);

  useFontMetricsResize(() => {
    const el = inputRef.current;
    if (el && !isTextareaHeightAnimating(el)) remeasureTextarea(el);
  });
  useWidthRemeasure(inputRef);

  useEffect(() => {
    const el = inputRef.current;
    return el ? attachDrawnCaret(el) : undefined;
  }, []);

  const [mention, setMention] = useState<{ query: string; start: number } | null>(null);
  const mentionThreadId = focusedThreadId.value;
  const mentionBackend = effectiveCodingAgentBackend(
    mentionThreadId ? threadMap.value.get(mentionThreadId) : undefined,
    resolveCodingAgent(mentionThreadId),
  );
  const mentionOffer = mention && mentionsEnabled.value
    ? mentionCandidates(mention.query, mentionableUsers.value, mentionBackend ? CODING_AGENT_NAME[mentionBackend] : LUCIDOS_AGENT_LABEL)
    : [];

  function autoResize() {
    const el = inputRef.current;
    if (el) resizeTextarea(el);
  }

  function writeComposerValue(el: HTMLTextAreaElement, text: string): void {
    el.value = text;
  }

  function recallPromptHistory(e: KeyboardEvent) {
    const threadId = focusedThreadId.value;
    const el = inputRef.current;
    if (!threadId || !el) return;
    const text = stepPromptHistory(
      threadId,
      {
        direction: e.key === 'ArrowUp' ? 'up' : 'down',
        text: el.value,
        selectionStart: el.selectionStart,
        selectionEnd: el.selectionEnd,
        modified: ['Meta', 'Control', 'Alt', 'Shift'].some((key) => e.getModifierState(key)),
      },
      sentPrompts(threadMap.peek().get(threadId)),
    );
    if (text === null) return;
    e.preventDefault();
    updateCompose(threadId, { text });
    requestPromptCaret(e.key === 'ArrowUp' ? 'start' : 'end');
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (isImeComposingKey(e)) return;
    if (e.key === 'Tab' && !e.getModifierState('Shift') && mentionOffer.length > 0) {
      e.preventDefault();
      pickMention(mentionOffer[0]);
      return;
    }
    if (e.key === 'Escape') {
      const action = computePromptEscapeAction(cancelTargetId !== null, isCancelSettling(), sideQuestionMode);
      if (action === 'leave-side-question') {
        e.preventDefault();
        setSideQuestionMode(false);
      } else if (action === 'cancel') {
        e.preventDefault();
        cancelExchangeForTarget();
      } else if (action === 'blur') {
        inputRef.current?.blur();
      }
      return;
    }
    if ((e.key === 'ArrowUp' || e.key === 'ArrowDown') && mentionOffer.length === 0) {
      recallPromptHistory(e);
      if (e.defaultPrevented) return;
    }
    if (composerEnterSends(e, currentComposerEnterKey(), isMobile())) {
      e.preventDefault();
      if (hasPendingMultiQ) void submitMultiAnswer();
      else void submit();
    }
  }

  function beginSend(
    threadId: string | null,
    thread: ThreadState | undefined,
    msg: string,
    currentImages: AttachedImage[],
    intent: UploadSendIntent<ChatContext>,
  ): Promise<void> {
    const imageHashes = currentImages.length > 0 ? currentImages.map((i) => i.hash) : undefined;
    const shouldFocus = threadId === null || focusedThreadId.value === threadId;

    const { promise: sendPromise, submittedId } = dispatchSend(threadId, () => {
      if (threadId && thread?.meta.state === 'composing') {
        return sendCompose(threadId, { useCodingAgent: intent.useCodingAgent, context: intent.context, focus: shouldFocus });
      } else if (threadId) {
        return sendFollowup(threadId, msg, imageHashes, { useCodingAgent: intent.useCodingAgent || undefined, context: intent.context, focus: shouldFocus });
      } else {
        return sendMessage(msg, imageHashes, { useCodingAgent: intent.useCodingAgent || undefined, context: intent.context, focus: shouldFocus });
      }
    });

    return sendPromise.catch((error) => {
      if (submittedId) {
        clearSubmittingThread(submittedId);
      }
      showToast('Failed to send message: ' + errorDetail(error), 'error');
    });
  }

  function sendQueuedAfterUpload(
    threadId: string,
    intent: UploadSendIntent<ChatContext>,
  ): Promise<void> {
    const thread = threadMap.value.get(threadId);
    if (!thread) {
      clearSubmittingThread(threadId);
      return Promise.resolve();
    }
    const draft = getDraft(threadId);
    const msg = thread.meta.state === 'composing' ? draft.text : draft.text.trim();
    const currentImages = getAttachedImages(threadId);
    if (!composeHasContent(msg, currentImages.length, false)) {
      showToast('The queued send was dropped: the composer is empty now.', 'info');
      clearSubmittingThread(threadId);
      return Promise.resolve();
    }
    const sideQuestion = routeSideQuestion(msg, {
      started: thread.meta.state !== 'composing',
    }, intent.asSideQuestion === true);
    if (sideQuestion.kind !== 'message') {
      clearSubmittingThread(threadId);
      if (sideQuestion.kind === 'refuse') showToast(sideQuestion.toast, 'info');
      else askComposerSideQuestion(threadId, sideQuestion.question, currentImages);
      return Promise.resolve();
    }
    const el = inputRef.current;
    if (el && el.dataset.threadId === threadId) {
      writeComposerValue(el, '');
      easeEmptiedTextarea(el);
    }
    return beginSend(threadId, thread, msg, currentImages, intent);
  }

  async function submit() {
    const el = inputRef.current;
    const threadId = focusedThreadId.value;
    const draftText = getDraft(threadId).text;
    const msg = draftText.trim();
    const currentImages = threadId ? getAttachedImages(threadId) : [];
    const uploadInFlight = threadId ? hasInFlightUploads(threadId) : false;
    if (!composeHasContent(msg, currentImages.length, uploadInFlight)) {
      if (draftText.length > 0) showToast('Nothing to send: the message is only spaces.', 'info');
      return;
    }
    const thread = threadId ? threadMap.value.get(threadId) : undefined;
    const useCodingAgent = effectiveSendMode(thread) === 'claude_code';
    const sideQuestion = routeSideQuestion(msg, {
      started: threadId !== null && thread !== undefined && thread.meta.state !== 'composing',
    }, sideQuestionMode);
    if (sideQuestion.kind === 'refuse') {
      showToast(sideQuestion.toast, 'info');
      return;
    }
    if (threadId && uploadsGate(threadId) === 'failed') {
      markUploadBlockedSend(threadId);
      return;
    }
    const context = currentChatContext();
    if (sideQuestion.kind === 'ask' && threadId) {
      endPromptHistory(threadId);
      if (uploadInFlight) {
        queueUploadSend(threadId, { useCodingAgent, context, asSideQuestion: true });
        return;
      }
      askComposerSideQuestion(threadId, sideQuestion.question, currentImages);
      return;
    }
    if (threadId && sendGateAsks(threadId) && !await passSendGate(threadId)) return;
    if (threadId) endPromptHistory(threadId);
    if (threadId && uploadInFlight) {
      armCancelSettle();
      queueUploadSend(threadId, { useCodingAgent, context });
      return;
    }
    if (el) writeComposerValue(el, '');
    const inComposeLayout = !threadId || thread?.meta.state === 'composing';
    if (inComposeLayout) {
      promptSendCollapsing.value = true;
    } else if (el) {
      easeEmptiedTextarea(el);
    }
    followSentMessage();
    if (isMobile()) el?.blur();

    armCancelSettle();
    await beginSend(threadId, thread, msg, currentImages, { useCodingAgent, context });
    restoreComposerFocus();
  }

  function restoreComposerFocus() {
    if (isMobile()) return;
    const active = document.activeElement;
    if (active && active !== document.body) return;
    focusIfNeeded(inputRef.current);
  }

  function askComposerSideQuestion(threadId: string, question: string, images: AttachedImage[]): void {
    armCancelSettle();
    const el = inputRef.current;
    if (el && el.dataset.threadId === threadId) {
      writeComposerValue(el, '');
      easeEmptiedTextarea(el);
    }
    const hashes = images.map((image) => image.hash);
    if (hashes.length > 0) markHashesAsSent(hashes);
    updateCompose(threadId, { text: '', image_hashes: [] });
    if (sideQuestionModeOn(threadId)) updateComposeSelection(threadId, { sideQuestionMode: false });
    followSideQuestion();
    void askSideQuestion(threadId, question, hashes);
    restoreComposerFocus();
  }

  function setSideQuestionMode(on: boolean): void {
    const threadId = focusedThreadId.value;
    if (!threadId) return;
    updateComposeSelection(threadId, { sideQuestionMode: on });
    if (on) focusIfNeeded(inputRef.current);
  }

  useSignalEffect(() => {
    void pendingUploads.value;
    settleQueuedUploadSends((threadId, intent) => {
      void sendQueuedAfterUpload(threadId, intent as UploadSendIntent<ChatContext>);
    });
  });

  function pickMention(candidate: MentionCandidate) {
    const el = inputRef.current;
    if (!el || !mention) return;
    const caret = el.selectionStart ?? el.value.length;
    el.setRangeText(`@${candidate.token} `, mention.start, caret, 'end');
    handleInput();
  }

  function handleInput() {
    autoResize();
    const el = inputRef.current;
    if (!el) return;
    const val = el.value;
    setMention(mentionsEnabled.value ? mentionQueryAt(val, el.selectionStart ?? val.length) : null);
    const tid = focusedThreadId.value;
    const thread = tid ? threadMap.value.get(tid) : undefined;
    const isClaudeCodeMode = effectiveCodingAgentBackend(thread, resolveCodingAgent(tid)) === 'claude-code';
    if (opensSlashMenu(val, isClaudeCodeMode)) {
      writeComposerValue(el, '');
      autoResize();
      codingAgentMenuOpenRequest.value = '';
      if (tid) updateCompose(tid, { text: '' });
      return;
    }
    const threadId = ensureFocusedComposeThread();
    updateCompose(threadId, { text: val });
  }

  function handlePaste(e: ClipboardEvent) {
    const items = e.clipboardData?.items;
    if (items) {
      for (let i = 0; i < items.length; i++) {
        const item = items[i];
        if (item.type.startsWith('image/')) {
          e.preventDefault();
          const file = item.getAsFile();
          if (!file) continue;
          addImageFile(file);
          return;
        }
      }
    }

    const el = inputRef.current;
    if (!el) return;
    const start = el.selectionStart;
    const end = el.selectionEnd;
    if (start === end) return;
    const text = e.clipboardData?.getData('text/plain') ?? '';
    const url = extractPasteUrl(text);
    if (!url) return;
    e.preventDefault();
    const selection = escapeMarkdownLinkText(el.value.slice(start, end));
    el.setRangeText(`[${selection}](${url})`, start, end, 'end');
    handleInput();
  }

  function handleFileSelect(e: Event) {
    const input = e.target as HTMLInputElement;
    if (!input.files) return;
    for (let i = 0; i < input.files.length; i++) {
      addImageFile(input.files[i]);
    }
    input.value = '';
  }

  function removeImage(index: number): void {
    const id = focusedThreadId.value;
    if (!id) return;
    removeAttachedImage(id, index);
  }

  const focusedThread = focusedThreadId.value ? threadMap.value.get(focusedThreadId.value) : undefined;
  const promptCodingAgent = effectiveCodingAgentBackend(
    focusedThread,
    resolveCodingAgent(focusedThreadId.value),
  );

  const showToggles = !focusedThreadId.value || focusedThread?.meta.state === 'composing';
  const [fading, setFading] = useState(false);

  useEffect(() => {
    if (!showToggles) {
      setFading(true);
      const t = setTimeout(
        () => setFading(false),
        scaledDurationMs(TOGGLES_FADE_MS) + TOGGLES_FADE_SLACK_MS,
      );
      return () => { clearTimeout(t); setFading(false); };
    }
  }, [showToggles]);

  const togglesMounted = showToggles || fading;
  const togglesFading = !showToggles && fading;

  const isNarrow = typeof window !== 'undefined' && window.innerWidth <= 600;
  const images = attachedImagesForCurrentThread.value;
  void pendingUploads.value;
  const focusedTid = focusedThreadId.value;
  const pending = focusedTid ? getPendingUploads(focusedTid) : [];
  const uploadsBlocking = focusedTid ? hasInFlightUploads(focusedTid) : false;
  const uploadSendQueued = focusedTid ? queuedUploadSends.value.has(focusedTid) : false;
  const uploadNotice = focusedTid
    ? uploadSendNotice(queuedUploadSends.value.get(focusedTid), uploadBlockedSends.value.has(focusedTid), pending)
    : null;
  const hasContent = composeHasContent(composeText, images.length, uploadsBlocking);
  void multiSelectedByToolUse.value;
  const pendingPicks = pendingAnswers.map.value;
  const focusedStatus = focusedThread ? effectiveThreadStatus(focusedThread) : 'idle';
  const isAnsweringQuestion = focusedStatus === 'waiting_for_user_answer';
  const rawPendingQ = isAnsweringQuestion ? findLatestPendingQuestion(focusedThread) : null;
  const pendingQ = rawPendingQ && !pendingPicks.has(rawPendingQ.toolUseId) ? rawPendingQ : null;
  const answeringQuestionCard = pendingQ !== null;
  const sideQuestionMode = sideQuestionModeActive({
    stored: sideQuestionModeOn(focusedThreadId.value),
    threadStarted: focusedThread !== undefined && focusedThread.meta.state !== 'composing',
  });
  const pausableThreadId = mentionsEnabled.value && focusedThread?.meta.state === 'active' ? focusedThreadId.value : null;
  const agentPaused = useAgentPaused(pausableThreadId);
  const placeholder = promptPlaceholder(threadHasConversation(focusedThreadId.value, focusedThread), answeringQuestionCard, sideQuestionMode, agentPaused);
  useEffect(() => {
    const el = inputRef.current;
    if (el) remeasureTextareaForPlaceholder(el);
  }, [placeholder]);
  const pendingMultiQ = pendingQ?.multiSelect ? pendingQ : null;
  const multiSelectedIds = pendingMultiQ ? getMultiSelectedIds(pendingMultiQ.toolUseId) : [];
  const hasPendingMultiQ = pendingMultiQ !== null && !sideQuestionMode;
  const morphHasContent = hasContent && !hasPendingMultiQ && !uploadSendQueued;
  const toggleMode = effectiveSendMode(focusedThread);
  const willUseCodingAgent = toggleMode === 'claude_code';
  const hasUrlContext = !!panelUrl.value && !willUseCodingAgent;
  const isComposingFocused = focusedThread?.meta.state === 'composing';
  const inComposeContext = isComposeContext(focusedThread);
  const codingAgentControlThreadId = focusedThread?.meta.state === 'active'
    ? focusedThreadId.value ?? undefined
    : undefined;
  const composeControlThreadId = isComposingFocused ? focusedThreadId.value ?? undefined : undefined;

  const waitingState = getWaitingState();

  const cancelTargetId =
    waitingState?.type === 'canceling' ? waitingState.threadId
    : (focusedTid && submittingThreadIds.value.has(focusedTid)) ? focusedTid
    : null;
  const isCanceling = cancelTargetId !== null && cancelingThreadIds.value.has(cancelTargetId);
  const bannerState = composerBannerState(waitingState, hasContent);

  const morphMode = computeMorphMode({
    hasContent: morphHasContent,
    cancelTargetId,
    isCanceling,
    hasBannerOrSectionButtons: !!bannerState,
  });

  const cancelSettling = isCancelSettling();

  const answersCard = isAnsweringQuestion && !(sideQuestionMode && morphHasContent);
  const answerMode = answersCard
    ? computeAnswerActionMode({
        isCanceling,
        hasContent: morphHasContent,
        multi: hasPendingMultiQ
          ? { pickedCount: multiSelectedIds.length, text: composeText, imageCount: images.length }
          : null,
      })
    : null;

  const sideQuestionRefusal = sideQuestionBlocker({
    threadStarted: focusedThread !== undefined && focusedThread.meta.state !== 'composing',
  });
  function toggleSideQuestionMode(): void {
    if (sideQuestionRefusal !== null) {
      showToast(sideQuestionRefusal, 'info');
      return;
    }
    setSideQuestionMode(!sideQuestionMode);
  }
  useSignalEffect(() => {
    if (!promptSideQuestionRequested.value) return;
    promptSideQuestionRequested.value = false;
    toggleSideQuestionMode();
  });
  const tapGateHandlers = {
    onPointerDown: (e: PointerEvent) => morphGate.down(e),
    onPointerMove: (e: PointerEvent) => morphGate.move(e),
    onPointerCancel: () => morphGate.cancel(),
  };
  const morphShownAt = useFaceShownAt(answersCard ? 'answer-control' : morphMode);
  const answerShownAt = useFaceShownAt(answerMode ?? 'none');
  const morphActivate = useTouchActivated(
    () => {
      if (morphMode === 'send') void submit();
      else if (morphMode === 'cancel') cancelExchangeForTarget();
    },
    morphMode === 'send' || (morphMode === 'cancel' && !cancelSettling),
    morphActivationGate,
    morphMode === 'cancel',
    morphShownAt,
  );
  const answerSubmitActivate = useTouchActivated(() => {
    if (hasPendingMultiQ) void submitMultiAnswer();
    else void submit();
  }, true, morphActivationGate);
  const answerCancelActivate = useTouchActivated(
    () => cancelExchangeForTarget(),
    !cancelSettling,
    morphActivationGate,
    true,
    answerShownAt,
  );

  useEffect(() => {
    const focused = focusedThreadId.value;
    if (!focused || !cancelingThreadIds.value.has(focused)) return;
    const thread = threadMap.value.get(focused);
    if (!thread) return;
    const canceledQid = canceledQuestionByThread.value.get(focused);
    const latestPendingQid = findLatestPendingQuestion(thread)?.toolUseId;
    const canceledWhileAwaiting = canceledWhileAwaitingByThread.value.has(focused);
    if (shouldClearCanceling(effectiveThreadStatus(thread), canceledQid, latestPendingQid, canceledWhileAwaiting)) {
      const next = new Set(cancelingThreadIds.value);
      next.delete(focused);
      cancelingThreadIds.value = next;
      setCanceledQuestion(focused, undefined);
      setCanceledWhileAwaiting(focused, false);
    }
  }, [focusedThreadId.value, threadMap.value]);

  useEffect(() => {
    const focused = focusedThreadId.value;
    if (!focused || !submittingThreadIds.value.has(focused)) return;
    const thread = threadMap.value.get(focused);
    if (!thread) return;
    if (shouldClearSubmitting(
      effectiveThreadStatus(thread),
      queuedUploadSends.value.has(focused),
    )) {
      const next = new Set(submittingThreadIds.value);
      next.delete(focused);
      submittingThreadIds.value = next;
    }
  }, [focusedThreadId.value, threadMap.value, queuedUploadSends.value]);

  useEffect(() => {
    const focused = focusedThreadId.value;
    if (!focused || !answeringThreadIds.value.has(focused)) return;
    const thread = threadMap.value.get(focused);
    if (!thread) return;
    if (thread.meta.status !== 'waiting_for_user_answer') {
      clearThreadAnswering(focused);
    }
  }, [focusedThreadId.value, threadMap.value]);

  async function submitMultiAnswer() {
    if (!pendingMultiQ) return;
    const focused = focusedThreadId.value;
    if (!focused) return;
    const el = inputRef.current;
    const text = getDraft(focused).text.trim();
    const ids = getMultiSelectedIds(pendingMultiQ.toolUseId);
    const gate = uploadsGate(focused);
    if (gate === 'failed') {
      markUploadBlockedSend(focused);
      return;
    }
    if (gate === 'in-flight') {
      showToast(UPLOAD_BLOCKS_ANSWER_TOAST, 'info');
      return;
    }
    const imageHashes = getAttachedImages(focused).map((image) => image.hash);
    if (ids.length === 0 && text.length === 0 && imageHashes.length === 0) return;
    armCancelSettle();
    const answer: AnswerKind = {
      kind: 'MultiSelected',
      option_ids: ids,
      ...(text.length > 0 ? { text } : {}),
      ...(imageHashes.length > 0 ? { image_hashes: imageHashes } : {}),
    };
    pendingAnswers.set(pendingMultiQ.toolUseId, answer);
    followAnsweredQuestion(pendingMultiQ.toolUseId);
    if (el) {
      writeComposerValue(el, '');
      easeEmptiedTextarea(el);
    }
    if (imageHashes.length > 0) markHashesAsSent(imageHashes);
    updateCompose(focused, { text: '', image_hashes: [] });
    setMultiSelectedIds(pendingMultiQ.toolUseId, []);
    if (isMobile()) el?.blur();
    const outcome = await answerThreadQuestion(focused, pendingMultiQ.toolUseId, answer);
    if (outcome === 'sent') discardUnsentAnswers(focused, pendingMultiQ.toolUseId);
    else pendingAnswers.clear(pendingMultiQ.toolUseId);
    if (outcome === 'refused') {
      const recovered = recoverableAnswerDraft({
        sentIds: ids,
        sentText: text,
        currentIds: getMultiSelectedIds(pendingMultiQ.toolUseId),
        currentDraft: getDraft(focused).text,
      });
      if (recovered.ids) setMultiSelectedIds(pendingMultiQ.toolUseId, recovered.ids);
      if (recovered.text !== null) updateCompose(focused, { text: recovered.text });
      if (imageHashes.length > 0 && getAttachedImages(focused).length === 0) {
        updateCompose(focused, { image_hashes: imageHashes });
      }
    }
    restoreComposerFocus();
  }

  useSignalEffect(() => {
    if (!promptStopRequested.value) return;
    promptStopRequested.value = false;
    untracked(cancelExchangeForTarget);
  });

  function cancelExchangeForTarget() {
    if (isCancelSettling()) return;
    const targetId = cancelTargetId;
    if (!targetId) return;
    const targetQuestionId = findLatestPendingQuestion(focusedThread)?.toolUseId;
    const canceledWhileAwaiting = focusedThread
      ? effectiveThreadStatus(focusedThread) === 'waiting_for_user_answer'
      : false;
    if (queuedUploadSends.value.has(targetId)) {
      clearQueuedUploadSend(targetId);
      setCanceledQuestion(targetId, undefined);
      setCanceledWhileAwaiting(targetId, false);
      return;
    }
    setCanceledQuestion(targetId, targetQuestionId);
    setCanceledWhileAwaiting(targetId, canceledWhileAwaiting);
    followCanceledTurn(canceledWhileAwaiting ? targetQuestionId : undefined);
    void handleCancelExchange(targetId);
  }

  const submitMultiCount = hasPendingMultiQ ? computeSubmitMultiCount(multiSelectedIds.length, composeText) : 0;
  const submitLabel = submitMultiCount > 0 ? `Submit ${submitMultiCount}` : 'Submit';
  const answerControl = answerMode === 'canceling' ? (
    <button key="answer-lone" type="button" class="action-btn action-btn-danger action-slot" disabled aria-label="Canceling" data-row-item>
      <SlotFace icon={() => <CloseIcon />} label="Cancel" busy />
    </button>
  ) : answerMode === 'submit' ? (
    <button
      key="answer-lone"
      type="button"
      class="action-btn action-btn-confirm action-slot"
      {...tapGateHandlers}
      onTouchStart={answerSubmitActivate.onTouchStart}
      onTouchMove={answerSubmitActivate.onTouchMove}
      onTouchCancel={answerSubmitActivate.onTouchCancel}
      onTouchEnd={answerSubmitActivate.onTouchEnd}
      onClick={answerSubmitActivate.onClick}
      aria-label="Submit answer"
      data-tooltip={uploadsBlocking ? 'Send after image upload' : 'Submit answer'}
      data-row-item
    >
      <SlotFace icon={() => <CheckIcon />} label={submitLabel} />
    </button>
  ) : answerMode === 'cancel' ? (
    <button
      key="answer-lone"
      type="button"
      class="action-btn action-btn-danger action-slot"
      disabled={cancelSettling}
      {...tapGateHandlers}
      onTouchStart={answerCancelActivate.onTouchStart}
      onTouchMove={answerCancelActivate.onTouchMove}
      onTouchCancel={answerCancelActivate.onTouchCancel}
      onTouchEnd={answerCancelActivate.onTouchEnd}
      onClick={answerCancelActivate.onClick}
      aria-label="Cancel"
      data-tooltip={answeringQuestionCard ? ANSWER_CANCEL_TOOLTIP : tooltipWithShortcut('Stop', 'stopThread')}
      data-row-item
    >
      <SlotFace icon={() => <CloseIcon />} label="Cancel" />
    </button>
  ) : null;

  const pendingRung = !inComposeContext && !promptCodingAgent && ['running', 'waiting_for_user_answer', 'paused'].includes(focusedStatus)
    && chatTurnSelection(focusedThreadId.value ?? undefined).pending;
  const rungDisabledReason = answersCard && promptCodingAgent
    ? 'Answer or cancel the pending question or permission request before changing the model and effort'
    : null;
  const bannerActions = bannerState ? getBannerActions(bannerState) : null;
  const situationalActions = bannerActions ? bannerActions.situational : getStandaloneActions();
  const sendButton = morphMode !== 'hidden' ? (
    <button
      key="send-cancel-morph"
      class={
        'action-btn send-cancel-morph action-slot'
        + (morphMode === 'cancel' && cancelSettling ? ' morph-settling' : '')
      }
      {...tapGateHandlers}
      onTouchStart={morphActivate.onTouchStart}
      onTouchMove={morphActivate.onTouchMove}
      onTouchCancel={morphActivate.onTouchCancel}
      onTouchEnd={morphActivate.onTouchEnd}
      onClick={morphActivate.onClick}
      aria-label={morphMode === 'cancel' || morphMode === 'canceling' ? 'Cancel'
        : sideQuestionMode ? 'Ask side question' : 'Send message'}
      tabIndex={morphMode === 'send' || morphMode === 'cancel' ? undefined : -1}
      disabled={
        morphMode === 'send' ? false
        : morphMode === 'cancel' ? cancelSettling
        : true
      }
      data-tooltip={
        morphMode === 'cancel' ? tooltipWithShortcut('Stop', 'stopThread')
        : morphMode === 'canceling' ? 'Stopping…'
        : morphMode === 'send' && uploadsBlocking ? 'Send after image upload'
        : morphMode === 'send' && sideQuestionMode ? 'Ask side question'
        : morphMode === 'send' ? 'Send'
        : undefined
      }
      data-row-item
    >
      {morphMode === 'cancel' || morphMode === 'canceling'
        ? <SlotFace icon={() => <StopIcon />} label="Stop" busy={morphMode === 'canceling'} />
        : <SlotFace icon={() => <SendArrowIcon />} label={sideQuestionMode ? 'Ask' : 'Send'} />}
    </button>
  ) : null;
  const actionSlot = answersCard ? answerControl
    : sendButton ?? (bannerActions && <ActionSlot occupant={bannerActions.slot} compact={composerCompact} />);
  const rowClass = bannerState
    ? 'prompt-actions-row thread-action-buttons'
    : 'prompt-actions-row';
  const readouts: HeaderActionSpec[] = [];
  const todoAction = promptCodingAgent === null ? todoIndicatorAction() : null;
  if (todoAction) readouts.push(todoAction);
  const waitingAction = waitingIndicatorAction();
  if (waitingAction) readouts.push(waitingAction);
  const attachImageAction: HeaderActionSpec = {
    key: 'attach-image',
    dataRole: 'attach-image',
    label: 'Attach image',
    tooltip: 'Attach image',
    icon: () => <ImageIcon />,
    onClick: () => fileInputRef.current?.click(),
    render: isNarrow ? (attrs) => (
      <button
        {...attrs}
        class="icon-btn header-icon"
        {...composeHandlers(() => fileInputRef.current?.click(), () => {})}
        data-tooltip="Attach image"
        aria-label="Attach image"
      >
        <ImageIcon />
      </button>
    ) : (attrs) => (
      <div class="image-attach-anchor" ref={menuRef} {...attrs}>
        <button
          class="icon-btn header-icon"
          {...composeHandlers(() => { attachMenuOpen.value = !attachMenuOpen.value; })}
          data-tooltip="Attach image"
          aria-label="Attach image"
        >
          <ImageIcon />
        </button>
        <Overlay
          open={attachMenuOpen.value}
          onClose={() => { attachMenuOpen.value = false; }}
          anchor={menuRef.current}
          backdrop={false}
          panelClass="surface-box image-attach-menu"
        >
          <button onClick={() => { attachMenuOpen.value = false; cameraOpen.value = true; }}>
            <CameraIcon />
            Camera
          </button>
          <button onClick={() => { attachMenuOpen.value = false; fileInputRef.current?.click(); }}>
            <FileIcon />
            File
          </button>
        </Overlay>
      </div>
    ),
  };
  const clearDraftAction: HeaderActionSpec | null = hasText ? {
    key: 'prompt-clear',
    extraClass: 'prompt-clear',
    label: 'Clear draft',
    icon: () => <ClearIcon />,
    onClick: () => {
      const el = inputRef.current;
      if (!el) return;
      writeComposerValue(el, '');
      const id = focusedThreadId.value;
      if (id) updateCompose(id, { text: '' });
      autoResize();
      el.focus();
    },
  } : null;
  const members = composerRowMembers({
    codingAgentMenu: promptCodingAgent === null ? null : codingAgentMenuAction(
      menuCodingAgent(promptCodingAgent, codingAgentControlThreadId, composeControlThreadId),
    ),
    attachImage: attachImageAction,
    readouts,
    wipPreview: wipPreviewAction(focusedThread),
    clearDraft: clearDraftAction,
    toggles: promptRowToggles(
      promptCodingAgent, focusedThread,
      sideQuestionRefusal === null ? sideQuestionToggleAction(sideQuestionMode, toggleSideQuestionMode) : null,
    ),
    rung: {
      key: RUNG_FOLD_KEY,
      dataRole: 'rung-control',
      label: pendingRung ? 'Model and effort *' : 'Model and effort',
      disabledTooltip: rungDisabledReason ?? undefined,
      icon: () => <RungMenuIcon />,
      onMenuClick: (anchor) => { if (anchor) rungFoldOpenRequest.value = anchor; },
    },
    situational: situationalActions,
  });
  const foldActions = members.foldOrder;

  const clusterKeys = new Set(members.cluster.map((a) => a.key));
  const foldSignature = foldActions.map((a) => a.key).join(' ');
  const foldKeys = useMemo(() => foldActions.map((a) => a.key), [foldSignature]);
  const foldGroups = useMemo<FoldGroup[]>(
    () => foldActions.map((a) => (clusterKeys.has(a.key) ? 'cluster' : 'row')),
    [foldSignature],
  );
  const pairAt = foldKeys.indexOf(DIFF_APPLY_MERGE.keys[0]);
  const mergeSpec = useMemo<PromptMerge | null>(
    () => (pairAt !== -1 && foldKeys[pairAt + 1] === DIFF_APPLY_MERGE.keys[1]
      ? { ...DIFF_APPLY_MERGE, atKey: members.mergeAt }
      : null),
    [foldSignature],
  );
  const { folded: collapsedActions, merged } = usePromptActionCollapse(
    promptActionsAreaRef, foldKeys, foldGroups, mergeSpec, composerCompact,
  );
  const hidden = foldActions.slice(0, collapsedActions);
  const foldedKeys = new Set(hidden.map((a) => a.key));
  const standing = (a: HeaderActionSpec) => !foldedKeys.has(a.key);
  const foldAttrs = (key: string) => ({ 'data-row-item': 'fold', [FOLD_KEY_ATTR]: key });
  const mountedWhileFolded: Record<string, (folded: boolean) => VNode> = {
    [CODING_AGENT_MENU_FOLD_KEY]: (folded) => (
      <LazyCodingAgentControlMenu
        threadId={codingAgentControlThreadId}
        composeThreadId={composeControlThreadId}
        codingAgent={promptCodingAgent}
        folded={folded}
        rowAttrs={foldAttrs(CODING_AGENT_MENU_FOLD_KEY)}
      />
    ),
    [RUNG_FOLD_KEY]: (folded) => (
      <LazyRungControl
        disabledReason={rungDisabledReason}
        threadId={promptCodingAgent === null ? focusedThreadId.value ?? undefined : codingAgentControlThreadId}
        composeThreadId={composeControlThreadId}
        composeContext={inComposeContext}
        codingAgent={promptCodingAgent}
        folded={folded}
        rowAttrs={foldAttrs(RUNG_FOLD_KEY)}
      />
    ),
  };
  const renderMember = (a: HeaderActionSpec) => {
    if (merged && mergeSpec && standing(a)) {
      if (a.key === mergeSpec.keys[0]) {
        const pair = foldActions.filter((m) => mergeSpec.keys.includes(m.key));
        return <Fragment key={a.key}><DiffApplyMenu members={pair} attrs={foldAttrs(mergeSpec.foldKey)} /></Fragment>;
      }
      if (a.key === mergeSpec.keys[1]) return null;
    }
    const mounted = mountedWhileFolded[a.key];
    if (mounted) return <Fragment key={a.key}>{mounted(foldedKeys.has(a.key))}</Fragment>;
    return standing(a) ? <Fragment key={a.key}>{renderHeaderAction(a, foldAttrs(a.key))}</Fragment> : null;
  };
  useEffect(closeFoldedPanels, [collapsedActions]);

  return (
    <div class="prompt-input-container">
      {(images.length > 0 || pending.length > 0) && (
        <div key="images" class="image-preview-strip">
          {images.map((img, i) => (
            <div class="image-preview-item" key={`hash-${img.hash}`}>
              <BlobImage
                src={img.previewUrl}
                class="image-preview-thumb"
                onClick={(e) => openImagePopupFromGroup(e.currentTarget.src, e.currentTarget)}
              />
              <button class="icon-btn image-preview-remove" onClick={() => removeImage(i)} aria-label="Remove" data-tooltip="Remove"><CloseIcon /></button>
            </div>
          ))}
          {pending.map((p) => (
            <PendingUploadChip
              key={`pending-${p.localId}`}
              upload={p}
              onOpen={(img) => openImagePopupFromGroup(img.src, img)}
              onRetry={() => void retryPendingUpload(p.threadId, p.localId)}
              onRemove={() => cancelPendingUpload(p.threadId, p.localId)}
            />
          ))}
        </div>
      )}
      {uploadNotice && (
        <div
          key="upload-notice"
          class={`upload-send-notice upload-send-notice-${uploadNotice.kind}`}
          data-role="upload-send-notice"
          role="status"
        >
          {uploadNotice.kind === 'queued' ? <span class="mini-spinner" aria-hidden="true" /> : <WarningIcon />}
          <span>{uploadSendNoticeText(uploadNotice)}</span>
        </div>
      )}
      {togglesMounted && <div key="toggles" class={`input-toggles-wrapper${togglesFading ? ' fading-out' : ''}`}>
        <ComposeDestinationRow
          threadId={focusedThreadId.value}
          toggleMode={toggleMode}
          fading={togglesFading}
        />
      </div>}
      {hasUrlContext && (
        <div class="url-context-pill" data-tooltip={panelUrl.value ?? undefined}>
          <GlobeIcon />
          <span class="url-context-label">{panelTitle.value || 'Page content'}</span>
        </div>
      )}
      <Disclosure key="side-question-mode" open={sideQuestionMode}>
        <div class="side-question-mode-pill" data-role="side-question-mode">
          <span>Side question</span>
          <button
            type="button"
            class="icon-btn side-question-mode-leave"
            aria-label="Back to a normal message"
            data-tooltip="Back to a normal message"
            onClick={() => setSideQuestionMode(false)}
          >
            <CloseIcon />
          </button>
        </div>
      </Disclosure>
      <MentionSuggestions key="mentions" candidates={mentionOffer} onPick={pickMention} />
      <HoldStrip key="hold" />
      <NotReadyStrip key="not-ready" />
      <SuccessorCard key="successor" onSend={() => void submit()} />
      <HandedOffCard key="handed-off" />
      <div key="prompt-box" class="prompt-box">
        <div class="prompt-row">
          <textarea
            ref={inputRef}
            class="prompt-textarea"
            data-role="prompt-input"
            data-thread-id={tid ?? ''}
            {...PROSE_TEXT_ATTRS}
            placeholder={placeholder}
            rows={1}
            onInput={handleInput}
            onKeyDown={handleKeyDown}
            onPaste={handlePaste}
            onFocus={() => focusPane('thread')}
            onBlur={() => setMention(null)}
          />
        </div>
        <input
          ref={fileInputRef}
          type="file"
          accept="image/*"
          multiple
          class="visually-hidden"
          tabIndex={-1}
          aria-hidden="true"
          onChange={handleFileSelect}
        />
        <div class={rowClass} ref={promptActionsAreaRef} data-composer-compact={composerCompact || undefined}>
          {hidden.length > 0 && (
            <OverflowMenu
              ariaLabel="More actions"
              extraClass="prompt-actions-more"
              triggerAttrs={MORE_TRIGGER_ATTRS}
              onOpen={closeRowPanels}
              items={(ctx) => hidden.map((a) => renderMenuAction(a, ctx))}
            />
          )}
          {members.row.map(renderMember)}
          <PromptActionsRight cluster={members.cluster.map(renderMember)} action={actionSlot} />
        </div>
      </div>
      <TodoPanelSlot />
      <WaitingPanelHost />
    </div>
  );
}
