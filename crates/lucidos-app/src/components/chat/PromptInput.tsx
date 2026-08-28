import { useRef, useEffect, useState, useMemo } from 'preact/hooks';
import { Overlay } from '../shared/Overlay';
import { signal, useSignalEffect } from '@preact/signals';
import { pendingChatMessage, showToast, openImagePopupFromGroup, focusedThreadId, threadMap, panelUrl, panelTitle, cancelingThreadIds, answeringThreadIds, clearThreadAnswering, effectiveThreadStatus, currentApp, wipPreviewThreadId, promptSendCollapsing, composeViewActive, scaledDurationMs } from '../../store/store';
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
import { followAnsweredQuestion, followCanceledTurn, followSentMessage } from './scrollState';
import { CaptureIcon, ImageIcon, CameraIcon, FileIcon, CloseIcon, ClearIcon, GlobeIcon, SendArrowIcon, StopIcon } from '../shared/icons';
import { BlobImage } from '../shared/BlobImage';
import { codingAgentMenuOpenRequest } from './CodingAgentControlMenu';
import { PromptRowControls } from './PromptRowControls';
import { getBannerSlots, getWaitingState, getStandaloneCcDiffButton, type BannerState } from './WaitingBanner';
import { composeHasContent, resolveComposerText, composerTextDisagreementToast, computeMorphMode, computeAnswerActionMode, computePromptEscapeAction, dispatchSend, computeSubmitMultiCount, findLatestPendingQuestion, promptPlaceholder, shouldClearCanceling, shouldClearSubmitting, submittingThreadIds, canceledQuestionByThread, setCanceledQuestion, canceledWhileAwaitingByThread, setCanceledWhileAwaiting, queuedUploadSends, queueUploadSend, takeQueuedUploadSend, clearQueuedUploadSend, clearSubmittingThread, armCancelSettle, isCancelSettling, type UploadSendIntent } from './prompt-input-helpers';
import { SplitButton } from '../shared/SplitButton';
export * from './prompt-input-helpers';
import { useFitsInOneRow } from '../../hooks/useFitsInOneRow';
import { composeHandlers } from './promptFocus';
import { focusIfNeeded } from '../../utils/dom';
import { threadEntryFocusTarget } from './choiceCardNav';
import { syncTextareaValue, shouldSkipSyncWhileEditing, promptOverrideSyncSeq, promptOverrideReplacesDraft } from './promptValueSync';
import { effectiveCodingAgentBackend, effectiveSendMode } from './promptToggleMode';
import { resizeTextarea, remeasureTextarea, isTextareaHeightAnimating, useFontMetricsResize, useWidthRemeasure, animateTextareaHeightFrom } from './promptResize';
import { isMobile } from '../../utils/viewport';
import { prefersReducedMotion } from '../../utils/platform';
import { createTapGate } from '../../utils/tapGesture';
import { useTouchActivated } from '../../hooks/useTouchActivated';
import { errorDetail } from '../../utils/errorDetail';
import { extractPasteUrl, escapeMarkdownLinkText } from '../../utils/extractPasteUrl';
import { PROSE_TEXT_ATTRS } from '../../utils/noAutofill';
import { attachedImagesForCurrentThread, getAttachedImages, removeAttachedImage, type AttachedImage } from './pastedImages';
import { getPendingUploads, hasInFlightUploads, removePendingUpload, pendingUploads } from '../../store/pendingUploads';
import { attachImageToActiveDraft } from './attachToDraft';
import { computeCaptureGeometry, readDeviceAngle } from './cameraGeometry';

const attachMenuOpen = signal(false);
const cameraOpen = signal(false);
/** 1x length of the compose-destination row's fade-out, mirroring
 *  `.input-toggles-wrapper`'s `transition: opacity var(--duration-slow)` in
 *  chat/input-messages.css. The literal is `--duration-slow` before the
 *  Animation speed slider scales it, so a timer on it goes through
 *  `scaledDurationMs`. */
const TOGGLES_FADE_MS = 300;
/** Fixed margin so the unmount lands AFTER the fade rather than on its last
 *  frame. Slack is a safety margin, not animation, so it stays outside the
 *  scaled call. */
const TOGGLES_FADE_SLACK_MS = 50;
const ANSWER_NO_IMAGES_TOAST = 'Answers to user questions are text only.';
/** Said when Send is pressed while an attached image is still uploading. The
 *  send is real and queued, so this reports a wait, not a refusal. */
const UPLOAD_QUEUED_SEND_TOAST = 'Sending once the image finishes uploading…';
const ANSWER_NO_IMAGES_TOOLTIP = 'Answers are text only';
/** Tooltip on the prompt row's Cancel while a question card is pending. Nothing
 *  else on screen spells out what the red button does to a pending question: it
 *  stamps the card `Canceled`, so the user can steer the agent elsewhere. The
 *  placeholder keeps the typing half (`PLACEHOLDER_ANSWERING`), and between the
 *  two nothing on the card needs an "Other, I'll type it" option.
 *
 *  Only while a question card is pending. The same button serves coding-agent
 *  permission cards, which are not `UserQuestionAsked` and absorb no typed
 *  text, so there it stays the plain "Stop". */
export const ANSWER_CANCEL_TOOLTIP = 'Cancel this question and ask something else';

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
    // Both failure paths below are real on iOS Safari. It refuses a new 2D
    // context, and can hand back a null blob, once its per-tab canvas memory
    // budget is spent. Neither may stay silent: the user pressed the shutter,
    // so an unhandled null leaves the button dead with the camera still open.
    // Both paths end in `close()`, as the success path does, since the shutter
    // is a one-shot. An early return that only toasts would strand the live
    // MediaStream and leave the overlay up.
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

  // Backdrop-only modal (the attach menu that opened it is gone by now, so
  // there is no anchor toggle) — <Overlay> owns dismiss/swallow/Escape/inert.
  return (
    <Overlay open onClose={close} overlayClass="camera-overlay" panelClass="camera-container" panelRole="dialog">
      <video ref={videoRef} autoPlay playsInline muted class="camera-video" />
      <div class="camera-controls">
        <button class="camera-capture-btn" onClick={capture} aria-label="Take photo" data-tooltip="Take photo">
          <CaptureIcon />
        </button>
        <button class="action-btn action-btn-danger" onClick={close}>Cancel</button>
      </div>
    </Overlay>
  );
}

// Pending uploads count as content. While a pasted or picked image is still
// uploading, the prompt is actively composing, so the waiting banner yields to
// the Send button. `computeMorphMode` reads `composeHasContent`, which includes
// pending uploads. Without this the banner's actions briefly show in place of
// Send during the upload window, for any thread in the review section.

export function PromptInput() {
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const promptActionsAreaRef = useRef<HTMLDivElement>(null);
  // Measure-driven stacking. The hook sums every `[data-row-item]`'s width and
  // compares against the row's content width. User font scaling, browser zoom
  // and per-thread label changes therefore feed in directly, with no
  // viewport-width heuristic to miss the squeeze on a dense row. When false,
  // the secondary candidate lifts to a row above the icons.
  //
  // `.prompt-actions-right` is the row's ONLY gapped cluster: the row itself
  // declares no `gap`, so its leading icon boxes touch. Naming the cluster
  // stops the check billing four gaps the row never spends. Those phantom gaps
  // lifted the Diff button off rows that could hold it.
  const fitsInOneRow = useFitsInOneRow(promptActionsAreaRef, {
    gappedCluster: '.prompt-actions-right',
  });
  // Scroll-vs-tap gate for the one-tap prompt buttons: the morph Send→Cancel
  // and the answer control's Submit / Cancel. An iOS PWA touch can stay under
  // iOS's ~10 px native cancel threshold during a scroll. It then lands a
  // `click` on whatever sits under the finger. Worst case that is the
  // destructive Cancel, which aborts the turn and stamps a pending question
  // `Canceled`.
  //
  // It therefore guards the CLICK path, which is where that stray click
  // arrives. `touchActivated` takes the gate and asks it there.
  //
  // The morph and the answer control are mutually exclusive, so they share one
  // gate instance. The multi-select split-button Submit needs no gate: its
  // caret menu makes the action deliberate. Each gated button wires the down,
  // move and cancel handlers inline rather than spreading a shared object,
  // because `prompt-cancel-tap-gate.test.ts` greps for that wiring.
  const morphGate = useMemo(() => createTapGate(), []);
  /** A discarded tap is the user's press thrown away, so it must never be
   *  silent: the button reads as dead and nothing says why.
   *
   *  Only the composer's own actions report it. A question-card option sits
   *  inside the transcript scroller. There, discarding a moving touch IS the
   *  gate doing its job, and a toast on every scroll starting on an option
   *  would be noise. */
  function morphTapPassed(): boolean {
    const moved = morphGate.tapRejection();
    if (moved === null) return true;
    showToast(`Tap ignored: it moved ${moved}px and read as a swipe. Try again.`, 'info');
    return false;
  }
  /** The gate as `touchActivated` takes it. A press the touch path served is
   *  spent, not ruled on: left unspent it would rule on the next activation
   *  with no press behind it.
   *
   *  `spend` is NOT `cancel`. Cancel means the system took the gesture, which
   *  `aborted` then reports, and a served press must not raise that flag. */
  const morphActivationGate = {
    pass: morphTapPassed,
    spend: morphGate.spend,
    aborted: morphGate.wasAborted,
  };
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
  // Whether the PREVIOUS render was the centered compose view. Drives the
  // compose-to-compose height animation, which must NOT fire on a
  // compose-to-active switch, where the ThreadPane FLIP owns the transition.
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
    // An empty canonical draft must reach the textarea even while it is
    // focused. Clearing never clobbers in-flight typing: `composeText` is ''
    // only when the draft is genuinely empty, since a keystroke updates it
    // synchronously. The skip guard protects non-empty in-flight content, and
    // letting it block an empty sync sticks stale text in a focused textarea.
    const forceEmptySync = composeText === '';
    // A one-shot override, a suggestion replacing an in-progress draft, must
    // land in the textarea whatever the focus and content are.
    // `requestPromptOverrideSync` bumps the counter after the draft write, so
    // this render sees both.
    const forceOverride = overrideSyncSeq !== lastOverrideSyncSeqRef.current;
    lastOverrideSyncSeqRef.current = overrideSyncSeq;
    // An override that REPLACED the draft end-snaps the caret, the same as a
    // thread switch: the old offset indexes text that is gone. An appending
    // override keeps it, since the prefix it points into is untouched.
    const preserveCursor = sameThread
      && !(forceOverride && promptOverrideReplacesDraft.value);
    if ((forceEmptySync || forceOverride || !shouldSkipSyncWhileEditing(el, sameThread, thisElementActive))
        && syncTextareaValue(el, composeText, preserveCursor)) {
      // A compose-view to compose-view switch keeps the centered layout put, so
      // the ThreadPane FLIP never fires and the textarea would insta-resize.
      // Ease its height from the previous view's to the new one instead.
      //
      // Gated on `composeViewActive`, NOT on both being composing threads: the
      // blank view has no thread id. A compose-to-active switch flips
      // `composeViewActive`, so this is false there and the FLIP owns it.
      // Capture the old inline height BEFORE `autoResize` overwrites it.
      // Desktop-only and motion-respecting, mirroring the ThreadPane FLIP.
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
      // A thread parked on a live choice card wants that card's default choice
      // focused, not the prompt, so Enter answers straight away.
      // `threadEntryFocusTarget` is the SINGLE place deciding between the two.
      // The card's own mount seed also fires on a switch, and letting both
      // decide independently would race on mount order.
      requestAnimationFrame(() => focusIfNeeded(threadEntryFocusTarget(el)));
    }
    prevTidRef.current = tid;
    wasComposeViewRef.current = isComposeView;
  }, [tid, composeText, overrideSyncSeq]);

  useFontMetricsResize(() => autoResize());
  useWidthRemeasure(inputRef);

  function autoResize() {
    const el = inputRef.current;
    if (el) resizeTextarea(el);
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      // Escape reaches the textarea only when no overlay is open: the central
      // overlay stack handles it first, in the capture phase, and stops
      // propagation (see .claude/rules/frontend.md). So here it belongs to the
      // composer, and means the same thing as the row's red button.
      const action = computePromptEscapeAction(cancelTargetId !== null, isCancelSettling());
      if (action === 'cancel') {
        e.preventDefault();
        cancelExchangeForTarget();
      } else if (action === 'blur') {
        inputRef.current?.blur();
      }
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
    if (!composeHasContent(msg, currentImages.length, false)) {
      // The user pressed Send and was told the send was waiting on the upload.
      // Emptying the box in the meantime cancels it, so say so rather than let
      // the promised send never arrive.
      showToast('The queued send was dropped: the composer is empty now.', 'info');
      clearSubmittingThread(threadId);
      return Promise.resolve();
    }
    if (effectiveThreadStatus(thread) === 'waiting_for_user_answer' && currentImages.length > 0) {
      showToast('Remove attached images to answer this question: answers are text only.', 'info');
      clearSubmittingThread(threadId);
      return Promise.resolve();
    }
    return beginSend(threadId, thread, msg, currentImages, intent);
  }

  async function submit() {
    const el = inputRef.current;
    const threadId = focusedThreadId.value;
    // ONE source for "is there anything to send": the draft the Send face was
    // rendered from, and the value `sendCompose` goes on to send. The textarea
    // only fills a gap the store has. See `resolveComposerText`.
    //
    // The node itself is no longer required. It is needed to clear the box and
    // to reset its height, and both sit under a null check below. A missing node
    // used to return here, which is a dead button that says nothing.
    const draftText = getDraft(threadId).text;
    const resolved = resolveComposerText(draftText, el ? el.value : null);
    const msg = resolved.text;
    const currentImages = threadId ? getAttachedImages(threadId) : [];
    const pendingForThread = threadId ? getPendingUploads(threadId) : [];
    const uploadInFlight = threadId ? hasInFlightUploads(threadId) : false;
    // The same reading the Send face was lit from, so a press on a LIT face can
    // never land here. What still reaches it is Enter on an empty desktop
    // composer, and there is nothing to say about that.
    //
    // A box holding characters is a different thing. Nothing sendable and
    // something on screen is the shape the user reports as a dead button, so it
    // says which it is. Every other return below dispatches or speaks.
    if (!composeHasContent(msg, currentImages.length, uploadInFlight)) {
      const onScreen = (el?.value.length ?? 0) > 0 || draftText.length > 0;
      if (onScreen) showToast('Nothing to send: the message is only spaces.', 'info');
      return;
    }
    // Only with a thread to hold a draft. Without one there is no stored copy,
    // so the box is the only source and there is nothing to disagree with.
    const disagreement = threadId ? composerTextDisagreementToast(resolved) : null;
    if (disagreement) showToast(disagreement, 'warning');
    // Before every return below, and before the dispatch: a queued upload send
    // re-reads the draft later, and `sendCompose` re-reads it now. Either would
    // otherwise carry the empty copy the recovery just repaired.
    if (threadId && resolved.storeWrite !== null) {
      updateCompose(threadId, { text: resolved.storeWrite });
    }
    // Backend reroutes typed text to the pending question's answer (see
    // chat/process/run.rs free-form path), but the answer payload drops images.
    // Refuse the send so the user can remove the images instead of silently
    // losing them. Disabling the attach buttons covers fresh attachments; this
    // catches images attached before the question opened.
    if (isAnsweringQuestion && (currentImages.length > 0 || pendingForThread.length > 0)) {
      showToast('Remove attached images to answer this question: answers are text only.', 'info');
      return;
    }
    const thread = threadId ? threadMap.value.get(threadId) : undefined;
    const useCodingAgent = effectiveSendMode(thread) === 'claude_code';
    const context = currentChatContext();
    if (threadId && uploadInFlight) {
      // A queued send still flips the button to the optimistic Cancel — settle.
      armCancelSettle();
      queueUploadSend(threadId, { useCodingAgent, context });
      // The one submit path that used to return with no message. The draft
      // stays put and the send fires later, which reads exactly like a dead
      // button. That is the shape this whole change is about, so say it.
      showToast(UPLOAD_QUEUED_SEND_TOAST, 'info');
      return;
    }
    if (el) el.value = '';
    // In the centered compose layout the prompt re-docks on send. The height
    // collapse defers to the ThreadPane FLIP, so a tall draft shrinks *and*
    // slides into the docked state together rather than snapping short first.
    // The FLIP consumes this flag and owns the reset in every path, so it
    // cannot stick tall. A docked follow-up send resets immediately.
    const inComposeLayout = !threadId || thread?.meta.state === 'composing';
    if (inComposeLayout) {
      promptSendCollapsing.value = true;
    } else if (el) {
      el.style.height = 'auto';
    }
    // Show the reader what they just wrote being picked up. That rests them on
    // the live edge, armed or not (ADR 0080). It covers a typed ANSWER too,
    // landing that on the card the text goes to. Here as well as in
    // `addPendingMessage`, because this is the composer's own tap and must not
    // wait on the awaited send below. Both calls describe the same submit, so
    // the second either keeps the first's request or restates it. A reader
    // already at the live edge is not scrolled, and a send into a thread
    // entirely on screen writes nothing.
    followSentMessage();
    if (isMobile()) el?.blur();

    // This constructive tap is about to morph the same button into the
    // destructive Cancel or Stop. Arm the settle window NOW, so a laggy repeat
    // tap cannot land on it. See `armCancelSettle`.
    armCancelSettle();
    await beginSend(threadId, thread, msg, currentImages, { useCodingAgent, context });
    restoreComposerFocus();
  }

  /** Put the caret back in the composer after a send. Sending is not leaving
   *  the composer: the next follow-up usually comes straight after, and on the
   *  compose→docked path the prompt is re-parented by the FLIP, which drops
   *  focus on its own. Mobile is excluded because it deliberately blurred on
   *  submit to drop the keyboard.
   *
   *  Only when nobody else has claimed focus meanwhile. The send is awaited, so
   *  by the time this runs the user may have clicked into another field. A
   *  question card that arrived seeds focus onto its own options. Neither
   *  should be yanked back. */
  function restoreComposerFocus() {
    if (isMobile()) return;
    const active = document.activeElement;
    if (active && active !== document.body) return;
    focusIfNeeded(inputRef.current);
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
    // Image paste needs `clipboardData.items`. The URL-on-selection
    // substitution below needs only `getData('text/plain')` and a selection.
    // Do NOT gate the whole handler on `items`: WebKit can deliver a paste with
    // usable `getData` but no items list, which would skip link substitution.
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
      // Keep the row mounted for the length of its own opacity transition
      // (`.input-toggles-wrapper`, `var(--duration-slow)`). Scaled by the
      // animation-speed slider, as that transition is. An unscaled timer
      // unmounts the row partway through a slowed fade, so the toggles pop
      // out instead of dissolving.
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
  // Subscribe so the strip re-renders when uploads settle.
  void pendingUploads.value;
  const focusedTid = focusedThreadId.value;
  const pending = focusedTid ? getPendingUploads(focusedTid) : [];
  const uploadsBlocking = focusedTid ? hasInFlightUploads(focusedTid) : false;
  const uploadSendQueued = focusedTid ? queuedUploadSends.value.has(focusedTid) : false;
  // The same reading `submit()` dispatches on. See `composeHasContent`: two
  // readings is what an enabled Send whose press does nothing is made of.
  const hasContent = composeHasContent(composeText, images.length, uploadsBlocking);
  void multiSelectedByToolUse.value;
  const pendingAnswers = pendingAnswerByToolUse.value;
  // Gate the exchange walk by status — without it, every keystroke would
  // sort + group all events. Suppress once optimistically answered so Submit
  // hides instead of flashing back as disabled.
  const focusedStatus = focusedThread ? effectiveThreadStatus(focusedThread) : 'idle';
  // While the thread waits for an answer, ANY text typed in the prompt becomes
  // a UserQuestion answer. Multi-select goes through `submitMultiAnswer` here.
  // Single-select and freetext are rerouted in chat/process/run.rs as
  // `AnswerKind::FreeText`, since the engine's fast path asks only whether the
  // user typed instead of clicking an option.
  //
  // The answer payload carries only text, so an image attached on this path
  // would be silently dropped. This flag refuses the attachment and toasts
  // instead, until `UserQuestionAnswered` grows an `image_hashes` field.
  const isAnsweringQuestion = focusedStatus === 'waiting_for_user_answer';
  // One exchange walk serves both consumers: the multi-select Submit control
  // and the placeholder. `waiting_for_user_answer` also covers coding-agent
  // permission cards. Those are NOT `UserQuestionAsked` and never absorb typed
  // text, so the placeholder keys off an actual pending question rather than
  // the status alone.
  const rawPendingQ = isAnsweringQuestion ? findLatestPendingQuestion(focusedThread) : null;
  // Drop an optimistically-answered question here, once, so every consumer
  // agrees the user is done with it: Submit hides AND the placeholder stops
  // inviting an answer during the click-to-SSE gap.
  const pendingQ = rawPendingQ && !pendingAnswers.has(rawPendingQ.toolUseId) ? rawPendingQ : null;
  const answeringQuestionCard = pendingQ !== null;
  // A placeholder swap changes what the empty box has to fit without touching
  // its value, which is all `resizeTextarea` reacts to. So each swap forces a
  // fresh measurement. The answering placeholder is the reason: it is the
  // longest of the three. A narrowed pane or a large UI scale wraps it where
  // the follow-up one does not, so the box grows to it and back.
  //
  // Except while the compose FLIP is easing the height. That animation inverts:
  // it parks the box at the height it came from, then transitions to the target
  // it already rests at. Writing the target here would land the box on it
  // before the transition starts, and the ease would play over zero distance.
  // Its target already accounts for the new placeholder, since the switch
  // effect above runs first.
  const placeholder = promptPlaceholder(!!focusedThreadId.value, answeringQuestionCard);
  useEffect(() => {
    const el = inputRef.current;
    if (el && !isTextareaHeightAnimating(el)) remeasureTextarea(el);
  }, [placeholder]);
  const pendingMultiQ = pendingQ?.multiSelect ? pendingQ : null;
  const multiSelectedIds = pendingMultiQ ? getMultiSelectedIds(pendingMultiQ.toolUseId) : [];
  const hasPendingMultiQ = pendingMultiQ !== null;
  // Submit consumes the typed text; queued upload sends also count as already
  // submitted so the normal Send→Cancel morph takes over while the hash lands.
  const morphHasContent = hasContent && !hasPendingMultiQ && !uploadSendQueued;
  // Coding agents don't use browser context — hide the pill when it won't be sent.
  const toggleMode = effectiveSendMode(focusedThread);
  const willUseCodingAgent = toggleMode === 'claude_code';
  const hasUrlContext = !!panelUrl.value && !willUseCodingAgent;
  // Per-draft coding-agent backend. Resolve the focused draft's override,
  // falling back to the global default. The control button and the slash
  // routing then follow the draft the user is editing.
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

  // Send, Cancel and the placeholder share ONE always-rendered <button> at the
  // same JSX position, so Preact never remounts it. The existing color
  // transition on `.action-btn` then animates the morph instead of a hard swap.
  // Cancel takes over when the thread has a cancel target: either the real
  // cancellable status from `getWaitingState`, or the optimistic submitting
  // flag bridging the click-to-SSE gap. Other `waitingState` types flow through
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

  // TOUCH ACTIVATION for the row's actions. The user presses these with the
  // mobile keyboard up. A tap then blurs the textarea, the keyboard starts
  // dismissing, and the button moves out from under the finger. WebKit drops
  // the synthetic click, so the press reads as dead with nothing on screen to
  // say why. `touchActivated` runs the action inside the gesture instead.
  //
  // The morph button is ONE node that turns destructive. It keeps the touch
  // path in both live modes. While it reads Cancel it passes `destructive`,
  // which makes that path rule on the tap gate rather than spend it. Withheld
  // entirely, the path left Cancel dead whenever the keyboard was up. See
  // `docs/plans/2026-08-28-cancel-survives-the-ios-keyboard.md`.
  //
  // The settle window is the other half of the guard, and `disabled` alone is
  // not it: a disabled element still receives touch events. So the destructive
  // faces stand the touch path down while `cancelSettling`.
  //
  // The constructive actions blur on their own (`submit`, `submitMultiAnswer`):
  // the suppressed click never reaches `installActionBtnBlurListener`, which
  // listens on `click`.
  const morphActivate = useTouchActivated(
    () => {
      if (morphMode === 'send') void submit();
      else if (morphMode === 'cancel') cancelExchangeForTarget();
    },
    morphMode === 'send' || (morphMode === 'cancel' && !cancelSettling),
    morphActivationGate,
    morphMode === 'cancel',
  );
  const answerSubmitActivate = useTouchActivated(() => {
    void submit();
  }, true, morphActivationGate);
  // The lone answer Cancel is its own node, so it needs its own activation.
  // Always destructive, and stood down for the same settle window.
  const answerCancelActivate = useTouchActivated(
    () => cancelExchangeForTarget(),
    !cancelSettling,
    morphActivationGate,
    true,
  );

  // Release the optimistic canceling flag once the cancel has landed. The set
  // survives component re-renders by design, since the button lives in the
  // always-visible prompt area. Without an explicit release the flag sticks
  // across the next stream and disables the button before it is pressed.
  //
  // `shouldClearCanceling` releases on EITHER the thread leaving every mid-turn
  // state OR the canceled question being replaced. The latter is the re-ask
  // case. An agent answering a cancel by re-asking keeps the thread mid-turn
  // throughout, so a not-mid-turn-only check would stick the button in a
  // disabled "Cancel..." until reload.
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

  // Mirror effect for the optimistic submitting flag, which covers the
  // click-to-SSE gap. Release it the moment either the real status takes over
  // OR nothing is in flight behind it. A Stop is then never offered on a thread
  // with no turn to stop. See `shouldClearSubmitting` for both arms.
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
    // See cancelingThreadIds effect above for why submittingThreadIds.value
    // is intentionally omitted from deps.
  }, [focusedThreadId.value, threadMap.value, queuedUploadSends.value]);

  // Release the optimistic answering flag once the real projection status
  // leaves `waiting_for_user_answer`. The resume is confirmed, or the turn
  // finished, so the real status can drive `isRenderedThreadIdle` from here.
  //
  // Read RAW `meta.status`, NOT `effectiveThreadStatus`. The flag itself is
  // what suppresses the false "Aborted", through `isRenderedThreadIdle`, so
  // gating on raw status keeps the release honest against the projection.
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
  // hides via the `!isAnsweringQuestion` render gate. But without this the
  // signal stays `true` and the menu pops back when the question resolves,
  // with no outside click to dismiss it.
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
    // The same one source as `submit`. This button's own count comes from
    // `computeSubmitMultiCount(..., composeText)`, which is the draft, so
    // reading the textarea here enabled it from one value and answered from
    // another. See `resolveComposerText`.
    const resolved = resolveComposerText(getDraft(focused).text, el ? el.value : null);
    const text = resolved.text;
    const disagreement = composerTextDisagreementToast(resolved);
    if (disagreement) showToast(disagreement, 'warning');
    const ids = getMultiSelectedIds(pendingMultiQ.toolUseId);
    if (ids.length === 0 && text.length === 0) return;
    // Once answered, pendingMultiQ clears and the row falls to the lone Cancel —
    // settle so a repeat tap can't abort the resuming turn. See armCancelSettle.
    armCancelSettle();
    const answer: AnswerKind = {
      kind: 'MultiSelected',
      option_ids: ids,
      ...(text.length > 0 ? { text } : {}),
    };
    setPendingAnswer(pendingMultiQ.toolUseId, answer);
    // Same ask as the composer's Send: hold the reader at the live edge while
    // the agent resumes, landing on what they just answered when they were not
    // already riding it. Before the awaited answer below, because this is the
    // button's own tap. A reader already at the live edge is not scrolled at
    // all, only armed.
    followAnsweredQuestion(pendingMultiQ.toolUseId);
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
      showToast('Could not send answer. Please try again.', 'error');
    }
    restoreComposerFocus();
  }

  const submitMultiCount = computeSubmitMultiCount(multiSelectedIds.length, composeText);
  const submitMultiDisabled = submitMultiCount === 0;

  // Cancel the current exchange: abort the turn, or stamp the pending question
  // Canceled. Shared by the morph button and the answer control's Cancel. It
  // snapshots the targeted question id, so the cleanup effect can release the
  // optimistic `cancelingThreadIds` flag even when the agent answers by
  // re-asking. A queued upload-send is dropped instead, having no live turn.
  function cancelExchangeForTarget() {
    // Within the post-submit settle window the destructive morph is held
    // disabled. This is the belt to the disabled prop's suspenders: a tap that
    // slips through, fired in the same frame before disabled applied, still
    // cannot abort the turn the user just started. See `armCancelSettle`.
    if (isCancelSettling()) return;
    const targetId = cancelTargetId;
    if (!targetId) return;
    const targetQuestionId = findLatestPendingQuestion(focusedThread)?.toolUseId;
    // Whether a card was on screen at click time. A permission card sets no
    // `canceledQuestionId`, not being an `UserQuestionAsked`. This bit is what
    // keeps such a cancel bridged through `waiting_for_user_answer` instead of
    // falling to the running-turn release. See `shouldClearCanceling`.
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
    // A submit like the other four, and taken BEFORE the awaited POST because
    // it is the button's own tap. Past the queued-upload return above, so a
    // cancel that sent the agent nothing moves nobody.
    //
    // The id is passed only while the thread is ACTUALLY awaiting an answer.
    // `findLatestPendingQuestion` has no liveness term, so it still answers with
    // a card the agent raced past or an abort stranded. Handed that, the landing
    // would hold on a turn that will never draw again. See `followCanceledTurn`.
    followCanceledTurn(canceledWhileAwaiting ? targetQuestionId : undefined);
    void handleCancelExchange(targetId);
  }

  // While the thread is `waiting_for_user_answer` the prompt row swaps the
  // morph Send/Stop for a Submit-default control (`computeAnswerActionMode`).
  // Multi-select is the only state needing the split button: its Submit is
  // always present, so Cancel lives behind the caret. Every other state is a
  // lone Submit, while a custom answer is typed, or a lone red Cancel. In the
  // second the forward action lives in the card above.
  const answerMode = isAnsweringQuestion
    ? computeAnswerActionMode({
        pendingMultiQ: hasPendingMultiQ,
        hasContent: morphHasContent,
        isCanceling,
      })
    : null;
  // The three lone-button states share ONE key ("answer-lone"), so crossing the
  // empty-to-typed boundary morphs Cancel and Submit in place rather than
  // remounting the node. That is the no-mobile-blink contract the morph button
  // keeps. Multi-select uses the SplitButton, its own node.
  const answerControl = answerMode === 'multi' ? (
    <SplitButton
      primaryLabel={submitMultiCount > 0 ? `Submit (${submitMultiCount})` : 'Submit'}
      primaryClassName="action-btn action-btn-confirm"
      primaryAriaLabel="Submit answer"
      primaryDisabled={submitMultiDisabled}
      primaryTouchActivate
      onPrimary={() => void submitMultiAnswer()}
      caretClassName="action-btn action-btn-confirm"
      caretAriaLabel="Cancel this question"
      menuItems={[{
        key: 'cancel',
        label: 'Cancel',
        className: 'action-btn action-btn-danger',
        // This path exists only while a multi-select question is pending, so the
        // question-specific wording always applies here.
        tooltip: ANSWER_CANCEL_TOOLTIP,
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
      onPointerDown={e => morphGate.down(e)}
      onPointerMove={e => morphGate.move(e)}
      onPointerCancel={() => morphGate.cancel()}
      onTouchEnd={answerSubmitActivate.onTouchEnd}
      onClick={answerSubmitActivate.onClick}
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
      // Held disabled for the post-submit settle window. The Submit the user
      // just pressed morphed into this Cancel, so a laggy repeat tap must not
      // abort the resuming turn. `cancelExchangeForTarget` belts the same check.
      disabled={cancelSettling}
      onPointerDown={e => morphGate.down(e)}
      onPointerMove={e => morphGate.move(e)}
      onPointerCancel={() => morphGate.cancel()}
      // The touch path runs the abort inside the gesture, because iOS drops
      // the click when the keyboard dismisses under the finger. Being
      // destructive, it rules on the gate: a press that travelled is refused
      // on both paths. It must never cancel `mousedown` to hold that click.
      // On iOS a cancelled event stops the rest of the synthesized sequence.
      onTouchEnd={answerCancelActivate.onTouchEnd}
      onClick={answerCancelActivate.onClick}
      aria-label="Cancel"
      // A pending question card gets the wording that says what Cancel does to
      // it; a permission card (same button, no typed-text escape) keeps "Stop".
      data-tooltip={answeringQuestionCard ? ANSWER_CANCEL_TOOLTIP : 'Stop'}
      data-row-item
    >
      Cancel
    </button>
  ) : null;

  // When the banner is suppressed, the in-banner Diff disappears with it. The
  // standalone Diff button fills that gap, so a branch with commits always
  // shows a Diff whatever the coding agent's run-state. It is the only liftable
  // slot while composing.
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
      onPointerDown={e => morphGate.down(e)}
      onPointerMove={e => morphGate.move(e)}
      onPointerCancel={() => morphGate.cancel()}
      onTouchEnd={morphActivate.onTouchEnd}
      onClick={morphActivate.onClick}
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
  // `.thread-action-buttons` is the e2e hook for a visible banner with action
  // buttons. Keep it bound to `bannerState`, so the selector flips with the
  // banner. The row wrapper itself always renders.
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
            placeholder={placeholder}
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
          <PromptRowControls
            codingAgent={promptCodingAgent}
            codingAgentThreadId={codingAgentControlThreadId}
            composeThreadId={composeControlThreadId}
            lucidosThreadId={focusedThreadId.value ?? undefined}
            composeContext={inComposeContext}
          />
          {/* The FOLLOW TOGGLE is the second item of `PromptRowControls` above,
              pinned there so it does not move between threads. Everything from
              here down is conditional and sits behind that fixed pair. */}
          {(() => {
            // WIP app preview toggle. Visible whenever the focused thread is an
            // app coding-agent thread with an in-flight diff.
            // `codingAgentHasDiff` is the same git-truth signal the Diff button
            // reads. It is cleared when the worktree is removed, so the toggle
            // can never point at a gone worktree.
            //
            // NOT gated on the app already being open. The preview swaps the
            // app's panel-overlay iframe, so gating it would leave a user
            // reviewing the change with the app closed unable to reach it.
            // Clicking ON opens the target app if needed, then flips that
            // iframe to the worktree-served WIP through the engine's
            // `?thread_id=<id>` route (`api/apps.rs::serve_app_ui`).
            //
            // Clicking OFF reverts to live, as does navigating away
            // (`actions/wipPreview.ts`) and an Apply or Discard removing the
            // worktree (the SSE handlers call `clearWipIfMatches`).
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
                {/* A filled eye, distinct from the outlined `EyeIcon` in
                    shared/icons.tsx. No inline width/height: the enclosing
                    `.icon-btn.header-icon` sizes the glyph from
                    `--icon-glyph`, so an attribute here is overridden. */}
                <svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
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
          {/* CLEAR THE DRAFT: the last icon of the row's left cluster, and
              deliberately not a second control on the right edge.

              It used to be pinned to the top-right corner of `.prompt-row`,
              which made the composer a two-corner composition with one row of
              controls. Three things were wrong with that and all three are
              positional. The corner ×'s centre sat 6px off the send's, because
              the two were inset by unrelated rules (its own `margin-right` vs
              `.prompt-actions-row`'s `padding-right`) at two different
              diameters, and circles are read by their centres. Their vertical
              distance was set by however tall the textarea happened to be, so
              nothing held the pair together. And the top-right corner is the
              universal "close this panel" slot, which is not what clearing a
              draft means.

              It carries no class of its own beyond the `.prompt-clear` hook the
              e2e specs select on: `.icon-btn.header-icon` is what gives it the
              box, the --icon-size-lg glyph, the --text-secondary gray and (via
              `.prompt-actions-row .icon-btn.header-icon`) the baseline nudge its
              neighbours ride. That is the point of the move rather than a
              side-effect of it. In the corner it drew a 14px --text-muted glyph
              where every other icon here is 20px --text-secondary, and the
              mobile override made it 22.5px, LARGER than the send, so the two
              controls' size relationship inverted between viewports.

              It renders only while there is a draft to clear. The row no
              longer reserves the box. An empty row spent 2.25rem on nothing,
              and on a phone that is what lifted the Diff button onto a row of
              its own. Reserving bought little: the banner leaves on the same
              keystroke this button arrives on, a far larger swing. Nothing on
              screen moves either way. This is the last item
              of the left cluster, and its next sibling has `margin-left: auto`,
              so mounting it only eats free space.

              Leaving `.prompt-row` also hands the textarea back its right
              content edge: as an in-flow flex sibling this button took its
              width, margin and the row gap out of the field in EVERY state,
              invisible ones included, so the typed text stopped 51px short of
              the box on the right against 13px on the left. */}
          {hasText && (
            <button
              key="prompt-clear"
              class="icon-btn header-icon prompt-clear"
              aria-label="Clear draft"
              data-tooltip="Clear draft"
              onClick={() => {
                const el = inputRef.current;
                if (!el) return;
                el.value = '';
                const id = focusedThreadId.value;
                if (id) updateCompose(id, { text: '' });
                autoResize();
                el.focus();
              }}
              data-row-item
            >
              <ClearIcon />
            </button>
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
