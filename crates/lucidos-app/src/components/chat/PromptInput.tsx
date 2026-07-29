import { useRef, useEffect, useState, useMemo } from 'preact/hooks';
import { Overlay } from '../shared/Overlay';
import { signal, useSignalEffect } from '@preact/signals';
import { pendingChatMessage, showToast, openImagePopupFromGroup, focusedThreadId, threadMap, panelUrl, panelTitle, cancelingThreadIds, answeringThreadIds, clearThreadAnswering, effectiveThreadStatus, isMidTurn, currentApp, wipPreviewThreadId, promptSendCollapsing, composeViewActive } from '../../store/store';
import { resolveCodingAgent } from '../../store/composeSelections';
import { sendMessage, handleCancelExchange } from '../../store/actions/chat';
import { currentChatContext, type ChatContext } from '../../store/actions/chatContext';
import { answerThreadQuestion } from '../../store/actions/chat-claude-code';
import { type AnswerKind, type ThreadState } from '../../store/thread-events';
import {
  multiSelectedByToolUse,
  pendingAnswerByToolUse,
  getMultiSelectedIds,
  setMultiSelectedIds,
  setPendingAnswer,
  clearPendingAnswer,
} from './QuestionCard';
import { updateCompose, sendCompose, sendFollowup, ensureFocusedComposeThread } from '../../store/actions/compose';
import { focusPane } from '../../store/actions/pane';
import { openAppById } from '../../store/actions/apps';
import { pushNavState } from '../../store/actions/navigation';
import { getDraft } from '../../store/composeDrafts';
import { ComposeDestinationRow } from './ComposeDestinationRow';
import { scrollToBottom, preserveAtBottom } from './scrollState';
import { CaptureIcon, ImageIcon, CameraIcon, FileIcon, CloseIcon, ClearIcon, GlobeIcon, SendArrowIcon, StopIcon } from '../shared/icons';
import { BlobImage } from '../shared/BlobImage';
import { CodingAgentControlMenu, codingAgentMenuOpenRequest } from './CodingAgentControlMenu';
import { LucidosControlMenu } from './LucidosControlMenu';
import { TodoListIndicator } from './TodoListPanel';
import { getBannerSlots, getWaitingState, getStandaloneCcDiffButton, type BannerState } from './WaitingBanner';
import { composeHasContent, computeMorphMode, computeAnswerActionMode, dispatchSend, computeSubmitMultiCount, findPendingMultiSelectQuestion, findLatestPendingQuestion, shouldClearCanceling, submittingThreadIds, canceledQuestionByThread, setCanceledQuestion, canceledWhileAwaitingByThread, setCanceledWhileAwaiting, queuedUploadSends, queueUploadSend, takeQueuedUploadSend, clearQueuedUploadSend, clearSubmittingThread, armCancelSettle, isCancelSettling, type UploadSendIntent } from './prompt-input-helpers';
import { SplitButton } from '../shared/SplitButton';
export * from './prompt-input-helpers';
import { useFitsInOneRow } from '../../hooks/useFitsInOneRow';
import { focusIfNeeded, composeHandlers } from './promptFocus';
import { syncTextareaValue, shouldSkipSyncWhileEditing, promptOverrideSyncSeq } from './promptValueSync';
import { effectiveCodingAgentBackend, effectiveSendMode } from './promptToggleMode';
import { resizeTextarea, useFontMetricsResize, animateTextareaHeightFrom } from './promptResize';
import { isMobile } from '../../utils/viewport';
import { prefersReducedMotion } from '../../utils/platform';
import { createTapGate } from '../../utils/tapGesture';
import { errorDetail } from '../../utils/errorDetail';
import { extractPasteUrl, escapeMarkdownLinkText } from '../../utils/extractPasteUrl';
import { PROSE_TEXT_ATTRS } from '../../utils/noAutofill';
import { attachedImagesForCurrentThread, getAttachedImages, removeAttachedImage, type AttachedImage } from './pastedImages';
import { getPendingUploads, hasInFlightUploads, removePendingUpload, pendingUploads } from '../../store/pendingUploads';
import { attachImageToActiveDraft } from './attachToDraft';
import { computeCaptureGeometry, readDeviceAngle } from './cameraGeometry';

const attachMenuOpen = signal(false);
const cameraOpen = signal(false);
const ANSWER_NO_IMAGES_TOAST = 'Answers to user questions are text only.';
const ANSWER_NO_IMAGES_TOOLTIP = 'Answers are text only';

function addImageFile(file: File) {
  attachImageToActiveDraft(file).catch((err) => {
    showToast('Failed to attach image: ' + errorDetail(err), 'error');
  });
}


function CameraCapture() {
  const videoRef = useRef<HTMLVideoElement>(null);
  const streamRef = useRef<MediaStream | null>(null);

  useEffect(() => {
    let canceled = false;
    navigator.mediaDevices.getUserMedia({ video: { facingMode: 'environment' } })
      .then((stream) => {
        if (canceled) { stream.getTracks().forEach((t) => t.stop()); return; }
        streamRef.current = stream;
        if (videoRef.current) {
          videoRef.current.srcObject = stream;
        }
      })
      .catch(() => {
        showToast('Could not access camera', 'error');
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
    const ctx = canvas.getContext('2d')!;
    ctx.translate(geom.translateX, geom.translateY);
    ctx.rotate(geom.rotateRadians);
    ctx.drawImage(video, 0, 0);
    canvas.toBlob((blob) => {
      if (blob) addImageFile(new File([blob], 'camera.jpg', { type: 'image/jpeg' }));
      streamRef.current?.getTracks().forEach((t) => t.stop());
      cameraOpen.value = false;
    }, 'image/jpeg', 0.9);
  }

  function close() {
    streamRef.current?.getTracks().forEach((t) => t.stop());
    cameraOpen.value = false;
  }

  // Backdrop-only modal (the attach menu that opened it is gone by now, so
  // there is no anchor toggle) — <Overlay> owns dismiss/swallow/Escape/inert.
  return (
    <Overlay open onClose={close} overlayClass="camera-overlay" panelClass="camera-container" panelRole="dialog">
      <video ref={videoRef} autoPlay playsInline muted class="camera-video" />
      <div class="camera-controls">
        <button class="camera-capture-btn" onClick={capture} data-tooltip="Take photo">
          <CaptureIcon />
        </button>
        <button class="action-btn action-btn-danger" onClick={close}>Cancel</button>
      </div>
    </Overlay>
  );
}

// Pending uploads count as content: while a pasted/picked image is still
// uploading, treat the prompt as actively composing so the waiting banner
// yields to the Send button (computeMorphMode reads composeHasContent, which
// includes pending uploads). Without this, the banner's actions briefly show in
// place of Send during the upload window for any thread in the review section.

export function PromptInput() {
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const promptActionsAreaRef = useRef<HTMLDivElement>(null);
  // Measure-driven stacking. The hook sums every [data-row-item]'s width and
  // compares against promptActionsAreaRef.clientWidth, so user font scaling,
  // browser zoom, and per-thread label changes (Apply ↔ Apply & Restart) all
  // feed in directly — no viewport-width heuristics that miss the squeeze on
  // dense rows. When false, the secondary candidate (the Diff button, in the
  // banner or standalone while composing) lifts to a row above the icons.
  const fitsInOneRow = useFitsInOneRow(promptActionsAreaRef);
  // Scroll-vs-tap gate for the one-tap prompt buttons — the morph Send→Cancel
  // and the answer control's Submit / Cancel. Without it, an iOS PWA touch that
  // stays under iOS's ~10 px native cancel threshold during a scroll lands a
  // `click` on whatever sits under the finger — worst case the destructive
  // Cancel that aborts the turn (and stamps any pending question as `Canceled`),
  // but a stray Submit firing mid-scroll is unwanted too. The morph and the
  // answer control are mutually exclusive (isAnsweringQuestion switches between
  // them), so they can share one gate instance. (The multi-select split-button
  // Submit needs no gate — its caret menu makes the action deliberate.) The
  // down/move/cancel handlers are wired inline on each gated button rather than
  // spread from a shared object — `prompt-cancel-tap-gate.test.ts` is a
  // source-text guard that greps for the literal wiring, so keep it visible.
  const morphGate = useMemo(() => createTapGate(), []);
  // Watch for pending messages from other modules (e.g. new app modal)
  useSignalEffect(() => {
    const msg = pendingChatMessage.value;
    if (!msg) return;
    pendingChatMessage.value = null;
    sendMessage(msg, undefined, { context: currentChatContext() }).catch((error) => {
      showToast('Failed to send message: ' + errorDetail(error), 'error');
    });
  });

  const tid = focusedThreadId.value;
  // Subscribe via composeDrafts, NOT threadMap: ChatExchange subscribes to
  // threadMap and runs marked.parse per render, so per-keystroke writes there
  // would re-parse every exchange in the thread.
  const composeText = getDraft(tid).text;
  const hasText = composeText.length > 0;

  // Preserve cursor on same-thread re-syncs; let it end-snap on thread switch.
  // shouldSkipSyncWhileEditing protects in-flight keystrokes — see its docstring.
  const prevTidRef = useRef<string | null | undefined>(undefined);
  // Whether the PREVIOUS render was the centered compose view — drives the
  // compose↔compose height animation (which must NOT fire on a compose↔active
  // switch, where the ThreadPane FLIP owns the transition instead).
  const wasComposeViewRef = useRef(false);
  // A deliberate programmatic override (welcome starter suggestion) bumps this
  // counter to force the very next sync past the skip-while-editing guard. Track
  // the last value we acted on so a bump forces exactly one sync.
  const overrideSyncSeq = promptOverrideSyncSeq.value;
  const lastOverrideSyncSeqRef = useRef(overrideSyncSeq);
  useEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    const sameThread = prevTidRef.current === tid;
    const isComposeView = composeViewActive.value;
    const thisElementActive = document.activeElement === el;
    // An empty canonical draft must reach the textarea even while it is focused:
    // clearing is never "clobbering in-flight typing" — composeText is '' only
    // when the draft is genuinely empty (a keystroke updates it synchronously).
    // The skip guard exists to protect non-empty in-flight content; letting it
    // block an empty sync left stale text stuck in a focused textarea after the
    // clear-X on WebKit (the "Clearing a follow-up draft" flake).
    const forceEmptySync = composeText === '';
    // A one-shot override (suggestion replacing an in-progress draft) must land
    // in the textarea regardless of focus/content — the drawer-updates-but-prompt-
    // stays-stale bug. requestPromptOverrideSync bumps the counter after the draft
    // write, so this render sees both.
    const forceOverride = overrideSyncSeq !== lastOverrideSyncSeqRef.current;
    lastOverrideSyncSeqRef.current = overrideSyncSeq;
    if ((forceEmptySync || forceOverride || !shouldSkipSyncWhileEditing(el, sameThread, thisElementActive))
        && syncTextareaValue(el, composeText, sameThread)) {
      // Compose-view → compose-view switch (e.g. draft ↔ draft, or draft ↔ the
      // blank compose view): the centered layout stays put, so the ThreadPane
      // FLIP never fires and the textarea would otherwise insta-resize. Ease its
      // height from the previous view's to the new one. Gate on composeViewActive
      // (NOT "both are composing threads" — the blank view has no thread id, which
      // is exactly the case the old gate missed). A compose↔active switch flips
      // composeViewActive, so this is false there and the FLIP owns it instead.
      // Capture the old inline height BEFORE autoResize overwrites it. Desktop-only
      // + motion-respecting, mirroring the ThreadPane FLIP.
      const animateSwitch = !sameThread && wasComposeViewRef.current && isComposeView
        && !isMobile() && !prefersReducedMotion();
      const fromHeight = animateSwitch ? el.style.height : '';
      autoResize();
      if (animateSwitch && fromHeight) {
        animateTextareaHeightFrom(el, fromHeight);
      } else {
        requestAnimationFrame(() => requestAnimationFrame(() => autoResize()));
      }
    }
    if (!sameThread && !isMobile()) {
      requestAnimationFrame(() => focusIfNeeded(el));
    }
    prevTidRef.current = tid;
    wasComposeViewRef.current = isComposeView;
  }, [tid, composeText, overrideSyncSeq]);

  useFontMetricsResize(() => autoResize());

  function autoResize() {
    const el = inputRef.current;
    if (!el) return;
    if (resizeTextarea(el)) preserveAtBottom();
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      inputRef.current?.blur();
      return;
    }
    if (e.key === 'Enter' && !e.shiftKey && !isMobile()) {
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
        // Composing thread: send through compose so server transitions
        // state→active and clears compose fields atomically.
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
    if (!msg.trim() && currentImages.length === 0) {
      clearSubmittingThread(threadId);
      return Promise.resolve();
    }
    if (effectiveThreadStatus(thread) === 'waiting_for_user_answer' && currentImages.length > 0) {
      showToast('Remove attached images to answer this question — answers are text only.', 'info');
      clearSubmittingThread(threadId);
      return Promise.resolve();
    }
    return beginSend(threadId, thread, msg, currentImages, intent);
  }

  async function submit() {
    const el = inputRef.current;
    if (!el) return;
    const msg = el.value.trim();
    const threadId = focusedThreadId.value;
    const currentImages = threadId ? getAttachedImages(threadId) : [];
    const pendingForThread = threadId ? getPendingUploads(threadId) : [];
    const uploadInFlight = threadId ? hasInFlightUploads(threadId) : false;
    if (!msg && currentImages.length === 0 && !uploadInFlight) return;
    // Backend reroutes typed text to the pending question's answer (see
    // chat/process.rs free-form path), but the answer payload drops images.
    // Refuse the send so the user can remove the images instead of silently
    // losing them. Disabling the attach buttons covers fresh attachments; this
    // catches images attached before the question opened.
    if (isAnsweringQuestion && (currentImages.length > 0 || pendingForThread.length > 0)) {
      showToast('Remove attached images to answer this question — answers are text only.', 'info');
      return;
    }
    const thread = threadId ? threadMap.value.get(threadId) : undefined;
    const useCodingAgent = effectiveSendMode(thread) === 'claude_code';
    const context = currentChatContext();
    if (threadId && uploadInFlight) {
      // A queued send still flips the button to the optimistic Cancel — settle.
      armCancelSettle();
      queueUploadSend(threadId, { useCodingAgent, context });
      preserveAtBottom();
      return;
    }
    el.value = '';
    // In the centered compose layout the prompt re-docks on send. Defer the
    // height collapse to the ThreadPane FLIP so a tall draft shrinks *and*
    // slides into the docked follow-up state together, instead of snapping
    // short first and then moving. The FLIP consumes this flag and owns the
    // reset in every path (incl. reduced-motion / mobile), so it can't stick
    // tall. Follow-up sends (docked already, no FLIP) reset immediately.
    const inComposeLayout = !threadId || thread?.meta.state === 'composing';
    if (inComposeLayout) {
      promptSendCollapsing.value = true;
    } else {
      el.style.height = 'auto';
    }
    scrollToBottom();
    if (isMobile()) el.blur();

    // This constructive tap is about to morph the same button into the
    // destructive Cancel/Stop (Send→Stop via the optimistic submitting flag, or
    // Submit→Cancel once the draft clears). Arm the settle window NOW so a laggy
    // repeat tap can't land on it. See armCancelSettle.
    armCancelSettle();
    await beginSend(threadId, thread, msg, currentImages, { useCodingAgent, context });
  }

  useSignalEffect(() => {
    const queued = queuedUploadSends.value;
    if (queued.size === 0) return;
    const uploads = pendingUploads.value;
    for (const [threadId] of queued) {
      const pendingForThread = uploads.get(threadId) ?? [];
      if (pendingForThread.some((u) => u.status === 'uploading')) continue;
      if (pendingForThread.some((u) => u.status === 'failed')) {
        clearQueuedUploadSend(threadId);
        continue;
      }
      const intent = takeQueuedUploadSend(threadId);
      if (!intent) continue;
      void sendQueuedAfterUpload(threadId, intent as UploadSendIntent<ChatContext>);
    }
  });

  function handleInput() {
    autoResize();
    // Typing the first character flips hasContent → the action row swaps
    // section buttons for Send, often changing prompt-actions-row
    // height even when the textarea itself didn't grow. Pin the user to the
    // bottom across the upcoming re-render so onResize can't escalate
    // scrolledUp=true on the layout shift.
    preserveAtBottom();
    const el = inputRef.current;
    if (!el) return;
    const val = el.value;
    // "/" prefix opens Claude Code slash commands. Codex shares the legacy
    // claude_code channel but has no slash-command surface, so Codex prompts
    // keep the slash as normal message text.
    const tid = focusedThreadId.value;
    const thread = tid ? threadMap.value.get(tid) : undefined;
    const isClaudeCodeMode = effectiveCodingAgentBackend(thread, resolveCodingAgent(tid)) === 'claude-code';
    if (isClaudeCodeMode && val.startsWith('/')) {
      el.value = '';
      autoResize();
      codingAgentMenuOpenRequest.value = val.slice(1);
      if (tid) updateCompose(tid, { text: '' });
      return;
    }
    const threadId = ensureFocusedComposeThread();
    updateCompose(threadId, { text: val });
  }

  function handlePaste(e: ClipboardEvent) {
    // Image paste needs clipboardData.items; the URL-on-selection substitution
    // below needs only getData('text/plain') + a selection. Don't gate the whole
    // handler on `items` — WebKit can deliver a paste (notably a synthesized one
    // in e2e) with usable getData but an empty/absent items list, which would
    // otherwise skip link substitution entirely.
    const items = e.clipboardData?.items;
    if (items) {
      for (let i = 0; i < items.length; i++) {
        const item = items[i];
        if (item.type.startsWith('image/')) {
          e.preventDefault();
          if (isAnsweringQuestion) {
            showToast(ANSWER_NO_IMAGES_TOAST, 'info');
            return;
          }
          const file = item.getAsFile();
          if (!file) continue;
          addImageFile(file);
          return; // Only process first image item
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
    // setRangeText keeps the change in the textarea's native undo stack.
    el.setRangeText(`[${selection}](${url})`, start, end, 'end');
    handleInput();
  }

  function handleFileSelect(e: Event) {
    const input = e.target as HTMLInputElement;
    if (!input.files) return;
    if (isAnsweringQuestion) {
      input.value = '';
      showToast(ANSWER_NO_IMAGES_TOAST, 'info');
      return;
    }
    for (let i = 0; i < input.files.length; i++) {
      addImageFile(input.files[i]);
    }
    input.value = ''; // Reset so same file can be selected again
  }

  function removeImage(index: number): void {
    const id = focusedThreadId.value;
    if (!id) return;
    removeAttachedImage(id, index);
  }

  const focusedThread = focusedThreadId.value ? threadMap.value.get(focusedThreadId.value) : undefined;

  // Toggle visibility: visible whenever the channel choice is mutable — the
  // compose view (no focused thread) AND a focused composing draft (state
  // hasn't locked to active yet). Derived inline so toggles mount immediately
  // on a state change — no useEffect sync needed.
  const showToggles = !focusedThreadId.value || focusedThread?.meta.state === 'composing';
  const [fading, setFading] = useState(false);

  useEffect(() => {
    if (!showToggles) {
      setFading(true);
      const t = setTimeout(() => setFading(false), 300);
      return () => { clearTimeout(t); setFading(false); };
    }
  }, [showToggles]);

  const togglesMounted = showToggles || fading;
  const togglesFading = !showToggles && fading;

  const isNarrow = typeof window !== 'undefined' && window.innerWidth <= 600;
  const images = attachedImagesForCurrentThread.value;
  // Subscribe so the strip re-renders when uploads settle.
  void pendingUploads.value;
  const focusedTid = focusedThreadId.value;
  const pending = focusedTid ? getPendingUploads(focusedTid) : [];
  const uploadsBlocking = focusedTid ? hasInFlightUploads(focusedTid) : false;
  const uploadSendQueued = focusedTid ? queuedUploadSends.value.has(focusedTid) : false;
  const hasContent = composeHasContent(hasText, images.length, pending.length);
  void multiSelectedByToolUse.value;
  const pendingAnswers = pendingAnswerByToolUse.value;
  // Gate the exchange walk by status — without it, every keystroke would
  // sort + group all events. Suppress once optimistically answered so Submit
  // hides instead of flashing back as disabled.
  const focusedStatus = focusedThread ? effectiveThreadStatus(focusedThread) : 'idle';
  // While the thread is waiting for an answer, ANY text typed in the prompt
  // becomes a UserQuestion answer — multi-select goes through submitMultiAnswer
  // here, single-select / freetext is intercepted in chat/process.rs and
  // rerouted as AnswerKind::FreeText. The answer payload only carries text, so
  // images attached on this path get silently dropped. Use this flag to refuse
  // image attachment + warn the user via toast until UserQuestionAnswered grows
  // an image_hashes field.
  const isAnsweringQuestion = focusedStatus === 'waiting_for_user_answer';
  const rawPendingMultiQ = isAnsweringQuestion
    ? findPendingMultiSelectQuestion(focusedThread)
    : null;
  const pendingMultiQ = rawPendingMultiQ && !pendingAnswers.has(rawPendingMultiQ.toolUseId)
    ? rawPendingMultiQ
    : null;
  const multiSelectedIds = pendingMultiQ ? getMultiSelectedIds(pendingMultiQ.toolUseId) : [];
  const hasPendingMultiQ = pendingMultiQ !== null;
  // Submit consumes the typed text; queued upload sends also count as already
  // submitted so the normal Send→Cancel morph takes over while the hash lands.
  const morphHasContent = hasContent && !hasPendingMultiQ && !uploadSendQueued;
  // Coding agents don't use browser context — hide the pill when it won't be sent.
  const toggleMode = effectiveSendMode(focusedThread);
  const willUseCodingAgent = toggleMode === 'claude_code';
  const hasUrlContext = !!panelUrl.value && !willUseCodingAgent;
  // Per-draft coding-agent backend: resolve the focused draft's override (falls
  // back to the global default) so the control button + slash routing match the
  // draft the user is editing, not a global another draft changed.
  const isComposingFocused = focusedThread?.meta.state === 'composing';
  // Compose context = a focused composing draft OR the fresh no-draft compose
  // view (no focused thread). NOT an active thread. Drives the control menus'
  // per-draft/pending routing so a fresh-compose pick lands in the pending slot,
  // never a global that every override-less draft reads.
  const inComposeContext = !focusedThread || focusedThread.meta.state === 'composing';
  const promptCodingAgent = effectiveCodingAgentBackend(
    focusedThread,
    resolveCodingAgent(focusedThreadId.value),
  );
  const showCodingAgentControls = promptCodingAgent !== null;
  // A focused composing draft has no backend session yet. Load controls as a
  // compose-view menu so Codex/Claude and repo scope come from the picker,
  // not from the server's legacy thread default. `codingAgentControlThreadId` is
  // the active-session id; `composeControlThreadId` is the composing draft id —
  // mutually exclusive, and the compose one keys the per-draft model/effort/scope.
  const codingAgentControlThreadId = focusedThread?.meta.state === 'active'
    ? focusedThreadId.value ?? undefined
    : undefined;
  const composeControlThreadId = isComposingFocused ? focusedThreadId.value ?? undefined : undefined;

  const waitingState = getWaitingState();

  // Send/Cancel/placeholder all share ONE always-rendered <button> at the
  // same JSX position so Preact never unmounts/remounts it — the existing
  // color transition on .action-btn animates the blue→red morph instead of
  // a hard swap. Cancel takes over when the thread has a cancel target —
  // either the real cancellable status from getWaitingState, or the
  // optimistic submitting flag (which bridges the click → SSE gap). Other
  // waitingState types (apply/discard/archive/actions) still flow through
  // WaitingBanner.
  const cancelTargetId =
    waitingState?.type === 'canceling' ? waitingState.threadId
    : (focusedTid && submittingThreadIds.value.has(focusedTid)) ? focusedTid
    : null;
  const isCanceling = cancelTargetId !== null && cancelingThreadIds.value.has(cancelTargetId);
  const bannerState: BannerState | null =
    !hasContent && waitingState && waitingState.type !== 'canceling'
      ? waitingState
      : null;

  const morphMode = computeMorphMode({
    hasContent: morphHasContent,
    cancelTargetId,
    isCanceling,
    hasBannerOrSectionButtons: !!bannerState,
  });

  // Post-submit settle: while true, the destructive Cancel/Stop morph renders
  // disabled so a laggy repeat tap can't abort the just-started turn. Read once
  // here so the render subscribes to the arm/expire signal transitions; used by
  // both the answer-control Cancel and the morph button below.
  const cancelSettling = isCancelSettling();

  // Release the optimistic canceling flag once the cancel has landed. The set
  // survives component re-renders by design (button lives in the always-visible
  // prompt area) — without explicit release the flag sticks across the next
  // stream and disables the button before it's pressed. `shouldClearCanceling`
  // releases on EITHER the thread leaving every mid-turn state OR the canceled
  // question being replaced/resolved. The latter is the re-ask case: canceling
  // a question whose cancel the agent answers by re-asking keeps the thread
  // mid-turn the whole time (waiting_for_user_answer → running →
  // waiting_for_user_answer), so a not-mid-turn-only check would leave the
  // button stuck in disabled "Cancel..." until reload.
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
    // cancelingThreadIds.value intentionally omitted from deps — the effect
    // writes to it, and it only needs to fire when status changes (carried by
    // threadMap). Including it would cause an extra no-op run after each clear.
  }, [focusedThreadId.value, threadMap.value]);

  // Mirror effect for the optimistic submitting flag: clear it once the
  // thread reaches a cancellable status. From that point on, the real
  // waitingState='canceling' drives the same morph button — the optimistic
  // flag has done its job (covered the click → SSE gap).
  useEffect(() => {
    const focused = focusedThreadId.value;
    if (!focused || !submittingThreadIds.value.has(focused)) return;
    const thread = threadMap.value.get(focused);
    if (!thread) return;
    if (isMidTurn(effectiveThreadStatus(thread))) {
      const next = new Set(submittingThreadIds.value);
      next.delete(focused);
      submittingThreadIds.value = next;
    }
    // See cancelingThreadIds effect above for why submittingThreadIds.value
    // is intentionally omitted from deps.
  }, [focusedThreadId.value, threadMap.value]);

  // Release the optimistic answering flag once the real projection status
  // leaves `waiting_for_user_answer` — the agent's resume is confirmed (status
  // → running) or the turn finished (→ idle), so the real status can drive
  // `isRenderedThreadIdle` from here. Read RAW `meta.status`, NOT
  // effectiveThreadStatus: the flag itself is what suppresses the false
  // "Aborted" via isRenderedThreadIdle (not effectiveThreadStatus), and gating
  // on raw status keeps the release honest against the live projection.
  useEffect(() => {
    const focused = focusedThreadId.value;
    if (!focused || !answeringThreadIds.value.has(focused)) return;
    const thread = threadMap.value.get(focused);
    if (!thread) return;
    if (thread.meta.status !== 'waiting_for_user_answer') {
      clearThreadAnswering(focused);
    }
    // See cancelingThreadIds effect above for why answeringThreadIds.value is
    // intentionally omitted from deps.
  }, [focusedThreadId.value, threadMap.value]);

  // Force-close the attach menu when a question arrives mid-open. The dropdown
  // already hides via the `!isAnsweringQuestion` render gate, but without this
  // the signal stays `true` and the menu would pop back the moment the
  // question resolves — without an outside click to dismiss it.
  useEffect(() => {
    if (isAnsweringQuestion && attachMenuOpen.value) attachMenuOpen.value = false;
  }, [isAnsweringQuestion]);

  // Bundles toggled option_ids + textarea text into one MultiSelected answer.
  // Backend joins them with `, ` for CC.
  async function submitMultiAnswer() {
    if (!pendingMultiQ) return;
    const focused = focusedThreadId.value;
    if (!focused) return;
    const el = inputRef.current;
    const text = el?.value.trim() ?? '';
    const ids = getMultiSelectedIds(pendingMultiQ.toolUseId);
    if (ids.length === 0 && text.length === 0) return;
    // Once answered, pendingMultiQ clears and the row falls to the lone Cancel —
    // settle so a repeat tap can't abort the resuming turn. See armCancelSettle.
    armCancelSettle();
    preserveAtBottom();
    const answer: AnswerKind = {
      kind: 'MultiSelected',
      option_ids: ids,
      ...(text.length > 0 ? { text } : {}),
    };
    setPendingAnswer(pendingMultiQ.toolUseId, answer);
    if (el) {
      el.value = '';
      el.style.height = 'auto';
    }
    updateCompose(focused, { text: '' });
    setMultiSelectedIds(pendingMultiQ.toolUseId, []);
    if (isMobile()) el?.blur();
    const ok = await answerThreadQuestion(focused, pendingMultiQ.toolUseId, answer);
    if (!ok) {
      // Drop optimistic so the question card un-resolves and the row
      // re-shows Submit. The toast tells the user to retry; not restoring the
      // cleared toggles + text avoids racing fresh input typed during the
      // failure window.
      clearPendingAnswer(pendingMultiQ.toolUseId);
      showToast('Could not send answer — please try again.', 'error');
    }
  }

  const submitMultiCount = computeSubmitMultiCount(multiSelectedIds.length, composeText);
  const submitMultiDisabled = submitMultiCount === 0;

  // Cancel the current exchange (abort the turn / stamp the pending question
  // Canceled). Shared by the morph button (running-turn Stop) and the answer
  // control's Cancel. Snapshots the targeted question id so the cleanup effect
  // can release the optimistic `cancelingThreadIds` flag even when the agent
  // answers the cancel by re-asking (thread stays mid-turn). A queued
  // upload-send is dropped instead — there's no live turn to interrupt yet.
  function cancelExchangeForTarget() {
    // Within the post-submit settle window the destructive morph is held
    // disabled; this is the belt to the disabled prop's suspenders, so a tap
    // that slips through (e.g. fired in the same frame before disabled applied)
    // still can't abort the turn the user just started. See armCancelSettle.
    if (isCancelSettling()) return;
    const targetId = cancelTargetId;
    if (!targetId) return;
    const targetQuestionId = findLatestPendingQuestion(focusedThread)?.toolUseId;
    // Whether a card (question OR permission) was on screen at click time.
    // A permission card sets no `canceledQuestionId` (it isn't an
    // UserQuestionAsked), so this bit is what keeps such a cancel bridged
    // through waiting_for_user_answer instead of falling to the running-turn
    // release. See `shouldClearCanceling`.
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
    void handleCancelExchange(targetId);
  }

  // While the thread is `waiting_for_user_answer` (pending question OR
  // permission) the prompt row swaps the morph Send/Stop for a Submit-default
  // control (computeAnswerActionMode). Multi-select is the only state that needs
  // the split button — its Submit is always present, so Cancel lives behind the
  // caret. Every other state is a lone Submit (while a freetext/custom answer is
  // typed) or a lone red Cancel (nothing to submit — the forward action lives in
  // the card above).
  const answerMode = isAnsweringQuestion
    ? computeAnswerActionMode({
        pendingMultiQ: hasPendingMultiQ,
        hasContent: morphHasContent,
        isCanceling,
      })
    : null;
  // The three lone-button states share ONE key ("answer-lone") so crossing the
  // empty↔typed boundary morphs Cancel↔Submit in place rather than
  // unmounting/remounting the node — the same no-mobile-blink contract the morph
  // button keeps. Multi-select uses the SplitButton (its own node).
  const answerControl = answerMode === 'multi' ? (
    <SplitButton
      primaryLabel={submitMultiCount > 0 ? `Submit (${submitMultiCount})` : 'Submit'}
      primaryClassName="action-btn action-btn-confirm"
      primaryAriaLabel="Submit answer"
      primaryDisabled={submitMultiDisabled}
      onPrimary={() => void submitMultiAnswer()}
      caretClassName="action-btn action-btn-confirm"
      caretAriaLabel="Cancel this question"
      menuItems={[{
        key: 'cancel',
        label: 'Cancel',
        className: 'action-btn action-btn-danger',
        tooltip: 'Stop',
        onClick: cancelExchangeForTarget,
      }]}
    />
  ) : answerMode === 'canceling' ? (
    <button key="answer-lone" type="button" class="action-btn action-btn-danger" disabled aria-label="Canceling" data-row-item>
      Canceling…
    </button>
  ) : answerMode === 'submit' ? (
    <button
      key="answer-lone"
      type="button"
      class="action-btn action-btn-confirm"
      onPointerDown={e => morphGate.down(e.clientX, e.clientY)}
      onPointerMove={e => morphGate.move(e.clientX, e.clientY)}
      onPointerCancel={() => morphGate.cancel()}
      onClick={() => { if (!morphGate.isTap()) return; void submit(); }}
      aria-label="Submit answer"
      data-tooltip={uploadsBlocking ? 'Send after image upload' : 'Send answer'}
      data-row-item
    >
      Submit
    </button>
  ) : answerMode === 'cancel' ? (
    // Lone destructive Cancel — keep the scroll-vs-tap gate so an iOS PWA scroll
    // can't land a one-tap abort (the concern that drove the morph gate).
    <button
      key="answer-lone"
      type="button"
      class="action-btn action-btn-danger"
      // Held disabled for the post-submit settle window so a laggy repeat tap
      // (the Submit the user just pressed morphed into this Cancel) can't abort
      // the resuming turn. cancelExchangeForTarget belts the same check.
      disabled={cancelSettling}
      onPointerDown={e => morphGate.down(e.clientX, e.clientY)}
      onPointerMove={e => morphGate.move(e.clientX, e.clientY)}
      onPointerCancel={() => morphGate.cancel()}
      onClick={() => { if (!morphGate.isTap()) return; cancelExchangeForTarget(); }}
      aria-label="Cancel"
      data-tooltip="Stop"
      data-row-item
    >
      Cancel
    </button>
  ) : null;

  // When the banner is suppressed (mid-turn 'canceling', or composing without
  // any waiting actions), the in-banner Diff disappears too. The standalone
  // Diff button fills that gap so "branch has commits → Diff visible" holds
  // regardless of CC's run-state — it's the only liftable slot while composing.
  const slots = bannerState
    ? getBannerSlots(bannerState)
    : { liftable: getStandaloneCcDiffButton(), primary: null };
  const stacked = !fitsInOneRow;
  const sendButton = morphMode !== 'hidden' ? (
    <button
      key="send-cancel-morph"
      class={
        'action-btn send-cancel-morph send-cancel-round'
        + (morphMode === 'placeholder' ? ' morph-placeholder' : '')
      }
      onPointerDown={e => morphGate.down(e.clientX, e.clientY)}
      onPointerMove={e => morphGate.move(e.clientX, e.clientY)}
      onPointerCancel={() => morphGate.cancel()}
      onClick={() => {
        if (!morphGate.isTap()) return;
        if (morphMode === 'send') void submit();
        else if (morphMode === 'cancel') cancelExchangeForTarget();
      }}
      aria-label={morphMode === 'cancel' || morphMode === 'canceling' ? 'Cancel' : 'Send message'}
      aria-hidden={morphMode === 'placeholder' ? 'true' : undefined}
      tabIndex={morphMode === 'send' || morphMode === 'cancel' ? undefined : -1}
      disabled={
        morphMode === 'send' ? false
        // Hold the just-morphed Stop disabled for the post-submit settle window
        // so a laggy repeat tap of Send can't immediately cancel the turn.
        : morphMode === 'cancel' ? cancelSettling
        : true
      }
      data-tooltip={
        morphMode === 'cancel' ? 'Stop'
        : morphMode === 'canceling' ? 'Stopping…'
        : morphMode === 'send' && uploadsBlocking ? 'Send after image upload'
        : morphMode === 'send' ? 'Send'
        : undefined
      }
      data-row-item
    >
      {/* Icon-only: an up-arrow for send, a stop-square while a turn is
          running/canceling. One stable element swaps only its glyph +
          aria-label/title between states — no unmount, so no mobile blink. */}
      {morphMode === 'cancel' || morphMode === 'canceling'
        ? <StopIcon />
        : <SendArrowIcon />}
    </button>
  ) : null;
  // .thread-action-buttons is the e2e-test hook for "the banner is visible
  // with action buttons" — keep it bound to bannerState so the selector still
  // flips when the banner appears/disappears, even though the row wrapper is
  // always rendered.
  const rowClass = bannerState
    ? 'prompt-actions-row thread-action-buttons'
    : 'prompt-actions-row';
  // `stacked` reflects only the measurement; lifting requires something to
  // lift. A row that overflows but has no liftable slot (e.g. the disabled
  // "Apply..." spinner during an apply turn) renders inline anyway, so the
  // is-stacked column layout would be wrong there.
  const isStacked = stacked && !!slots.liftable;
  const rightClass = isStacked
    ? 'prompt-actions-right is-stacked'
    : 'prompt-actions-right';

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
            <div class={`image-preview-item image-preview-pending image-preview-pending-${p.status}`} key={`pending-${p.localId}`}>
              <BlobImage
                src={p.previewUrl}
                class="image-preview-thumb"
                onClick={(e) => openImagePopupFromGroup(e.currentTarget.src, e.currentTarget)}
              />
              <button
                class="icon-btn image-preview-remove"
                onClick={() => focusedTid && removePendingUpload(focusedTid, p.localId)}
                aria-label={p.status === 'failed' ? 'Remove failed upload' : 'Cancel upload'}
                data-tooltip={p.status === 'failed' ? 'Remove' : 'Cancel'}
              ><CloseIcon /></button>
            </div>
          ))}
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
      <div key="prompt-box" class="prompt-box">
        <div class="prompt-row">
          <textarea
            ref={inputRef}
            class="prompt-textarea"
            data-role="prompt-input"
            data-thread-id={tid ?? ''}
            {...PROSE_TEXT_ATTRS}
            placeholder={focusedThreadId.value ? 'Post a follow up…' : 'What can I help with?'}
            rows={1}
            onInput={handleInput}
            onKeyDown={handleKeyDown}
            onPaste={handlePaste}
            // Writing in the prompt means working in the chat — keep the focused
            // pane in sync however focus arrived (click, type-to-focus, Tab,
            // programmatic). focusPane is signal-only + no-op on mobile, so this
            // can't loop with the pane-focus DOM-focus logic.
            onFocus={() => focusPane('thread')}
          />
          <button
            class={`icon-btn prompt-clear${hasText ? '' : ' invisible'}`}
            aria-label="Clear"
            onClick={() => {
              const el = inputRef.current;
              if (!el) return;
              el.value = '';
              const id = focusedThreadId.value;
              if (id) updateCompose(id, { text: '' });
              autoResize();
              el.focus();
            }}
          >
            <ClearIcon />
          </button>
        </div>
        {/* Single hidden file input lives at the top of prompt-box so the menu
            open/close re-render never unmounts it mid-tap. Photo buttons below
            trigger via `.click()` (the proven pattern from 0.7.2). Hidden via
            `.visually-hidden` (off-screen, in layout) — `display:none` is what
            HiddenFileInput's docs blame for dropping the iOS PWA change event,
            so we match that even though .click() is a synthetic dispatch. */}
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
        <div class={rowClass} ref={promptActionsAreaRef}>
          {showCodingAgentControls
            ? <CodingAgentControlMenu threadId={codingAgentControlThreadId} composeThreadId={composeControlThreadId} codingAgent={promptCodingAgent} />
            : <><LucidosControlMenu threadId={focusedThreadId.value ?? undefined} composeContext={inComposeContext} /><TodoListIndicator /></>}
          {(() => {
            // WIP app preview toggle: visible whenever the focused thread is an
            // app coding-agent thread that has an in-flight diff
            // (codingAgentHasDiff — the same git-truth signal as the Diff
            // button, cleared on Apply/Discard/Archive when the worktree is
            // removed, so the toggle can never point at a gone worktree). It is
            // NOT gated on the app already being open: that was a chicken-and-egg
            // bug — the preview swaps the app's panel-overlay iframe, so a user
            // reviewing the change with the app closed could never reach it.
            // Clicking ON opens the target app in the panel-overlay if it isn't
            // already, then flips that iframe to the worktree-served WIP via the
            // engine's `?thread_id=<id>` route (see api/apps.rs::serve_app_ui).
            // Clicking OFF reverts to live. The toggle also reverts to live when
            // the user navigates away (cleared by the focusedThreadId effect in
            // actions/wipPreview.ts) or when Apply/Discard removes the worktree
            // (thread-sync SSE handlers call clearWipIfMatches).
            const ft = focusedThread;
            if (!ft || ft.meta.codingAgentKind !== 'app') return null;
            if (!ft.meta.codingAgentHasDiff) return null;
            const folder = ft.meta.codingAgentFolder;
            const appId = folder ? folder.split('/').filter(Boolean).pop() : undefined;
            if (!appId) return null;
            const wipOn = wipPreviewThreadId.value === ft.meta.id;
            return (
              <button
                class={`icon-btn header-icon${wipOn ? ' active' : ''}`}
                data-tooltip={wipOn ? 'Showing the WIP app preview from this thread’s worktree. Click to return to the live app.' : 'Preview the in-flight changes from this app coding-agent thread in the panel.'}
                aria-pressed={wipOn}
                aria-label={wipOn ? 'Stop WIP app preview' : 'Show WIP app preview'}
                onClick={() => {
                  if (wipOn) {
                    // Revert to live. pushNavState captures wipPreviewThreadId
                    // into the new entry, so flip the signal first.
                    wipPreviewThreadId.value = null;
                    pushNavState();
                    return;
                  }
                  // Turning WIP on. The preview swaps the target app's
                  // panel-overlay iframe, so open that app first if it isn't the
                  // one currently shown. Set the WIP signal only AFTER the app
                  // is in place — otherwise the wipPreview effect would see a
                  // currentApp/wipApp mismatch and immediately clear it.
                  void (async () => {
                    if (currentApp.value?.id !== appId) {
                      await openAppById(appId);
                      if (currentApp.value?.id !== appId) return; // open failed — toast already shown
                    }
                    wipPreviewThreadId.value = ft.meta.id;
                    pushNavState();
                  })();
                }}
                data-row-item
                data-role="wip-preview-toggle"
              >
                {/* eye icon — keep it inline so we don't pull in another svg file */}
                <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
                  <path d="M8 3C4.5 3 1.7 5.3 0.5 8c1.2 2.7 4 5 7.5 5s6.3-2.3 7.5-5c-1.2-2.7-4-5-7.5-5zm0 8a3 3 0 1 1 0-6 3 3 0 0 1 0 6zm0-1.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3z"/>
                </svg>
              </button>
            );
          })()}
          {isNarrow ? (
            <button
              class="icon-btn header-icon"
              onClick={() => fileInputRef.current?.click()}
              aria-label="Attach image"
              disabled={isAnsweringQuestion}
              data-tooltip={isAnsweringQuestion ? ANSWER_NO_IMAGES_TOOLTIP : undefined}
              data-row-item
            >
              <ImageIcon />
            </button>
          ) : (
            <div class="image-attach-anchor" ref={menuRef} data-row-item>
              {/* composeHandlers keeps the prompt textarea focused (iPad PWA
                  keyboard stays open) so the menu anchors to the right spot —
                  see 5ca953fd7. */}
              <button
                class="icon-btn header-icon"
                {...composeHandlers(() => { attachMenuOpen.value = !attachMenuOpen.value; })}
                disabled={isAnsweringQuestion}
                data-tooltip={isAnsweringQuestion ? ANSWER_NO_IMAGES_TOOLTIP : 'Attach image'}
                aria-label="Attach image"
              >
                <ImageIcon />
              </button>
              <Overlay
                open={attachMenuOpen.value && !isAnsweringQuestion}
                onClose={() => { attachMenuOpen.value = false; }}
                anchor={menuRef.current}
                backdrop={false}
                panelClass="image-attach-menu"
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
          )}
          <div class={rightClass}>
            {isStacked ? (
              <>
                <div class="prompt-actions-subrow">
                  {slots.liftable}
                </div>
                <div class="prompt-actions-subrow">
                  {slots.primary}
                  {isAnsweringQuestion ? answerControl : sendButton}
                </div>
              </>
            ) : (
              <>
                {slots.liftable}
                {slots.primary}
                {isAnsweringQuestion ? answerControl : sendButton}
              </>
            )}
          </div>
        </div>
      </div>
      {cameraOpen.value && <CameraCapture />}
    </div>
  );
}
