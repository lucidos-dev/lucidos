import { useRef, useEffect, useState, useMemo } from 'preact/hooks';
import { useDismissOnOutside } from '../../hooks/useAnchoredPopover';
import { signal, useSignalEffect } from '@preact/signals';
import { pendingChatMessage, showToast, inputMode, openImagePopupFromGroup, focusedThreadId, threadMap, repositories, selectedScope, appsList, panelUrl, panelTitle, cancelingThreadIds, effectiveThreadStatus, isMidTurn, type Scope, currentApp, wipPreviewThreadId } from '../../store/store';
import { loadApps } from '../../store/actions/apps';
import { sendMessage, loadRepositories, handleCancelExchange } from '../../store/actions/chat';
import { currentChatContext } from '../../store/actions/chatContext';
import { handleSaveThread, handleUnsaveThread } from '../../store/actions/threads';
import { answerThreadQuestion } from '../../store/actions/chat-claude-code';
import { type AnswerKind } from '../../store/thread-events';
import {
  multiSelectedByToolUse,
  pendingAnswerByToolUse,
  getMultiSelectedIds,
  setMultiSelectedIds,
  setPendingAnswer,
  clearPendingAnswer,
} from './QuestionCard';
import { updateCompose, sendCompose, sendFollowup, ensureFocusedComposeThread, type ComposeMode } from '../../store/actions/compose';
import { pushNavState } from '../../store/actions/navigation';
import { getDraft } from '../../store/composeDrafts';
import { scrollToBottom, preserveAtBottom } from './scrollState';
import { CaptureIcon, ImageIcon, CameraIcon, FileIcon, CloseIcon, ClearIcon, GlobeIcon } from '../shared/icons';
import { Dropdown } from '../shared/Dropdown';
import { CCControlMenu, ccMenuOpenRequest } from './CCControlMenu';
import { TodoListIndicator } from './TodoListPanel';
import { getBannerSlots, getWaitingState, getStandaloneCcDiffButton, type BannerState } from './WaitingBanner';
import { composeHasContent, computeMorphMode, dispatchSend, computeSubmitMultiCount, findPendingMultiSelectQuestion, findLatestPendingQuestion, shouldClearCanceling, shouldLiftSectionButtons, submittingThreadIds, canceledQuestionByThread, setCanceledQuestion } from './prompt-input-helpers';
import { resolveThreadActions, discardDraft } from '../../store/actions/threadActions';
export * from './prompt-input-helpers';
import { useFitsInOneRow } from '../../hooks/useFitsInOneRow';
import { focusIfNeeded, composeHandlers } from './promptFocus';
import { syncTextareaValue, shouldSkipSyncWhileEditing } from './promptValueSync';
import { effectiveSendMode } from './promptToggleMode';
import { resizeTextarea, useFontMetricsResize } from './promptResize';
import { isMobile } from '../../utils/viewport';
import { createTapGate } from '../../utils/tapGesture';
import { errorDetail } from '../../utils/errorDetail';
import { extractPasteUrl, escapeMarkdownLinkText } from '../../utils/extractPasteUrl';
import { attachedImagesForCurrentThread, getAttachedImages, removeAttachedImage } from './pastedImages';
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

  return (
    <div class="camera-overlay" onClick={close}>
      <div class="camera-container" onClick={(e) => e.stopPropagation()}>
        <video ref={videoRef} autoPlay playsInline muted class="camera-video" />
        <div class="camera-controls">
          <button class="camera-capture-btn" onClick={capture} data-tooltip="Take photo">
            <CaptureIcon />
          </button>
          <button class="action-btn action-btn-danger" onClick={close}>Cancel</button>
        </div>
      </div>
    </div>
  );
}

// Pending uploads count as content: while a pasted/picked image is still
// uploading, treat the prompt as actively composing so the section buttons
// (Save/Archive) and waiting banner yield to Send + Discard. Without this,
// the Save chip briefly appears in place of Send during the upload window
// for any thread in the review section.

function SavePromptButton({ threadId }: { threadId: string }) {
  return (
    <button
      class="action-btn action-btn-confirm save-thread-btn"
      onClick={() => void handleSaveThread(threadId)}
      aria-label="Save thread"
      data-row-item
    >
      Save
    </button>
  );
}

// Mid-turn replacement for Archive in the Saved section. Click confirms then
// drops the thread out of Saved without canceling — once it idles it routes
// to Active → Review like any other running thread.
function UnsaveSavedPromptButton({ threadId }: { threadId: string }) {
  return (
    <button
      class="action-btn action-btn-confirm save-thread-btn"
      onClick={() => void handleUnsaveThread(threadId)}
      aria-label="Remove thread from Saved section"
      data-row-item
    >
      ✓ Saved
    </button>
  );
}

export function PromptInput() {
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const promptActionsAreaRef = useRef<HTMLDivElement>(null);
  // Measure-driven stacking. The hook sums every [data-row-item]'s width and
  // compares against promptActionsAreaRef.clientWidth, so user font scaling,
  // browser zoom, and per-thread label changes (Apply ↔ Apply & Restart) all
  // feed in directly — no viewport-width heuristics that miss the squeeze on
  // dense rows. When false, the secondary candidate (Diff for the banner,
  // Discard draft for compose) lifts to a row above the icons.
  const fitsInOneRow = useFitsInOneRow(promptActionsAreaRef);
  // Scroll-vs-tap gate for the morph Send→Cancel button. Without it, an iOS
  // PWA touch that stays under iOS's ~10 px native cancel threshold during a
  // scroll lands a `click` on whatever sits under the finger — and the morph
  // button in `waiting_for_user_answer` is a destructive Cancel that aborts
  // the turn and stamps the pending question as `Canceled`.
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
  useEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    const sameThread = prevTidRef.current === tid;
    const thisElementActive = document.activeElement === el;
    if (!shouldSkipSyncWhileEditing(el, sameThread, thisElementActive)
        && syncTextareaValue(el, composeText, sameThread)) {
      autoResize();
      requestAnimationFrame(() => requestAnimationFrame(() => autoResize()));
    }
    if (!sameThread && !isMobile()) {
      requestAnimationFrame(() => focusIfNeeded(el));
    }
    prevTidRef.current = tid;
  }, [tid, composeText]);

  useFontMetricsResize(() => autoResize());

  useDismissOnOutside(attachMenuOpen.value, menuRef, null, () => {
    attachMenuOpen.value = false;
  });

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

  async function submit() {
    const el = inputRef.current;
    if (!el) return;
    const msg = el.value.trim();
    const threadId = focusedThreadId.value;
    const currentImages = threadId ? getAttachedImages(threadId) : [];
    if (!msg && currentImages.length === 0) return;
    // Backend reroutes typed text to the pending question's answer (see
    // chat/process.rs free-form path), but the answer payload drops images.
    // Refuse the send so the user can remove the images instead of silently
    // losing them. Disabling the attach buttons covers fresh attachments; this
    // catches images attached before the question opened.
    if (isAnsweringQuestion && currentImages.length > 0) {
      showToast('Remove attached images to answer this question — answers are text only.', 'info');
      return;
    }
    const thread = threadId ? threadMap.value.get(threadId) : undefined;
    el.value = '';
    el.style.height = 'auto';
    scrollToBottom();
    if (isMobile()) el.blur();

    const useClaudeCode = effectiveSendMode(thread) === 'claude_code';

    const imageHashes = currentImages.length > 0 ? currentImages.map((i) => i.hash) : undefined;

    const context = currentChatContext();

    const { promise: sendPromise, submittedId } = dispatchSend(threadId, () => {
      if (threadId && thread?.meta.state === 'composing') {
        // Composing thread: send through compose so server transitions
        // state→active and clears compose fields atomically.
        return sendCompose(threadId, { useClaudeCode, context });
      } else if (threadId) {
        return sendFollowup(threadId, msg, imageHashes, { useClaudeCode: useClaudeCode || undefined, context });
      } else {
        return sendMessage(msg, imageHashes, { useClaudeCode: useClaudeCode || undefined, context });
      }
    });

    sendPromise.catch((error) => {
      if (submittedId) {
        const next = new Set(submittingThreadIds.value);
        next.delete(submittedId);
        submittingThreadIds.value = next;
      }
      showToast('Failed to send message: ' + errorDetail(error), 'error');
    });
  }

  async function handleDiscard() {
    const el = inputRef.current;
    if (!el) return;
    const id = focusedThreadId.value;
    if (!id) return;
    // The confirm + the active-vs-composing branch both live on the action
    // (discardDraft), so this button confirms identically to the close-cascade
    // shortcut — confirmation is tied to the action, never to how it's invoked.
    // Bail without touching the textarea or focus when the user cancels.
    if (!(await discardDraft(id))) return;
    el.value = '';
    el.style.height = 'auto';
    // Discard is an exit from compose — drop focus so the mobile keyboard
    // goes down.
    el.blur();
  }

  function handleInput() {
    autoResize();
    // Typing the first character flips hasContent → the action row swaps
    // section buttons for Send/Discard, often changing prompt-actions-row
    // height even when the textarea itself didn't grow. Pin the user to the
    // bottom across the upcoming re-render so onResize can't escalate
    // scrolledUp=true on the layout shift.
    preserveAtBottom();
    const el = inputRef.current;
    if (!el) return;
    const val = el.value;
    // "/" prefix in CC mode opens command menu with filter
    const tid = focusedThreadId.value;
    const thread = tid ? threadMap.value.get(tid) : undefined;
    const isCCMode = effectiveSendMode(thread) === 'claude_code';
    if (isCCMode && val.startsWith('/')) {
      el.value = '';
      autoResize();
      ccMenuOpenRequest.value = val.slice(1);
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

  /** Mode toggle. For a focused composing thread, persist the choice on the
   *  thread row so peers see it. Otherwise (no thread or active thread) just
   *  set the inputMode signal — there's nothing server-side to update. */
  function setMode(mode: ComposeMode) {
    inputMode.value = mode === 'claude_code' ? { type: 'claude_code' } : { type: 'do' };
    const id = focusedThreadId.value;
    if (!id) return;
    const thread = threadMap.value.get(id);
    if (thread?.meta.state !== 'composing') return;
    updateCompose(id, { mode });
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
  // Submit consumes the typed text — keep the morph in 'cancel' even with content.
  const morphHasContent = hasContent && !hasPendingMultiQ;
  // CC doesn't use browser context — hide the pill when it won't be sent
  const toggleMode = effectiveSendMode(focusedThread);
  const willUseClaudeCode = toggleMode === 'claude_code';
  const hasUrlContext = !!panelUrl.value && !willUseClaudeCode;
  const showCCCommands = willUseClaudeCode;

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

  // sectionButtonNodes carries ONLY the save/unsave toggle — Archive and the
  // change actions are rendered by WaitingBanner from the same selector. The
  // save-category action (Save vs ✓ Saved) is derived from
  // resolveThreadActions so this button can never drift from the selector that
  // the close cascade and the server-side guards also consult.
  const saveAction = tid && focusedThread
    ? resolveThreadActions(tid).find((a) => a.category === 'save')
    : undefined;
  const sectionButtonNodes = saveAction && tid
    ? [
        saveAction.kind === 'unsave'
          ? <UnsaveSavedPromptButton key="unsave" threadId={tid} />
          : <SavePromptButton key="save" threadId={tid} />,
      ]
    : null;

  const morphMode = computeMorphMode({
    hasContent: morphHasContent,
    cancelTargetId,
    isCanceling,
    hasBannerOrSectionButtons: !!bannerState || !!sectionButtonNodes,
  });

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
    if (shouldClearCanceling(effectiveThreadStatus(thread), canceledQid, latestPendingQid)) {
      const next = new Set(cancelingThreadIds.value);
      next.delete(focused);
      cancelingThreadIds.value = next;
      setCanceledQuestion(focused, undefined);
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
  const submitMultiButton = pendingMultiQ ? (
    <button
      key="submit-multi"
      type="button"
      class="action-btn action-btn-confirm"
      disabled={submitMultiDisabled}
      onClick={submitMultiAnswer}
      aria-label="Submit answer"
      data-row-item
    >
      {submitMultiCount > 0 ? `Submit (${submitMultiCount})` : 'Submit'}
    </button>
  ) : null;

  const composeDiscardButton = hasContent && !bannerState ? (
    <button
      key="discard-draft"
      class="action-btn action-btn-danger"
      onClick={handleDiscard}
      aria-label="Discard draft"
      data-row-item
    >
      Discard draft
    </button>
  ) : null;
  // sectionButtons (Save / ✓ Saved) are rendered separately below so they
  // anchor to the bottom row instead of lifting alongside Diff / Discard draft
  // when the row stacks.
  // When the banner is suppressed (mid-turn 'canceling', or composing without
  // any waiting actions), the in-banner Diff disappears too. The standalone
  // Diff button fills that gap so "branch has commits → Diff visible" holds
  // regardless of CC's run-state. Discard-draft wins when the user is
  // actively composing (only one liftable slot exists).
  const slots = bannerState
    ? getBannerSlots(bannerState)
    : { liftable: composeDiscardButton ?? getStandaloneCcDiffButton(), primary: null };
  const stacked = !fitsInOneRow;
  const sendButton = morphMode !== 'hidden' ? (
    <button
      key="send-cancel-morph"
      class={
        'action-btn send-cancel-morph'
        + (morphMode === 'cancel' || morphMode === 'canceling' ? ' action-btn-danger' : '')
        + (morphMode === 'placeholder' ? ' morph-placeholder' : '')
      }
      onPointerDown={e => morphGate.down(e.clientX, e.clientY)}
      onPointerMove={e => morphGate.move(e.clientX, e.clientY)}
      onPointerCancel={() => morphGate.cancel()}
      onClick={() => {
        if (!morphGate.isTap()) return;
        if (morphMode === 'send') void submit();
        else if (morphMode === 'cancel') {
          // Capture the question this cancel targets (if any) so the cleanup
          // effect can release the optimistic flag once it resolves — even when
          // the agent answers the cancel by re-asking (thread stays mid-turn).
          setCanceledQuestion(cancelTargetId!, findLatestPendingQuestion(focusedThread)?.toolUseId);
          void handleCancelExchange(cancelTargetId!);
        }
      }}
      aria-label={morphMode === 'cancel' || morphMode === 'canceling' ? 'Cancel' : 'Send message'}
      aria-hidden={morphMode === 'placeholder' ? 'true' : undefined}
      tabIndex={morphMode === 'send' || morphMode === 'cancel' ? undefined : -1}
      disabled={
        morphMode === 'send' ? uploadsBlocking
        : morphMode === 'cancel' ? false
        : true
      }
      data-tooltip={morphMode === 'send' && uploadsBlocking ? 'Waiting for image upload…' : undefined}
      data-row-item
    >
      {morphMode === 'canceling' ? 'Cancel...'
        : morphMode === 'cancel' ? 'Cancel'
        : 'Send'}
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
  const liftSection = shouldLiftSectionButtons(isStacked, bannerState);
  const rightClass = isStacked
    ? 'prompt-actions-right is-stacked'
    : 'prompt-actions-right';

  return (
    <div class="prompt-input-container">
      {(images.length > 0 || pending.length > 0) && (
        <div key="images" class="image-preview-strip">
          {images.map((img, i) => (
            <div class="image-preview-item" key={`hash-${img.hash}`}>
              <img
                src={img.previewUrl}
                class="image-preview-thumb"
                onClick={(e) => openImagePopupFromGroup(e.currentTarget.src, e.currentTarget)}
              />
              <button class="icon-btn image-preview-remove" onClick={() => removeImage(i)} aria-label="Remove" data-tooltip="Remove"><CloseIcon /></button>
            </div>
          ))}
          {pending.map((p) => (
            <div class={`image-preview-item image-preview-pending image-preview-pending-${p.status}`} key={`pending-${p.localId}`}>
              <img
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
      <div class="input-target-tabs segmented-control">
        <button
          class={`segmented-btn ${toggleMode === 'lucidos' ? 'active' : ''}`}
          {...composeHandlers(() => setMode('lucidos'))}
        >
          Lucidos
        </button>
        <button
          class={`segmented-btn ${toggleMode === 'claude_code' ? 'active' : ''}`}
          {...composeHandlers(() => setMode('claude_code'))}
        >
          Claude
        </button>
      </div>
        {!togglesFading && toggleMode === 'claude_code' && (() => {
          const reposLoadable = repositories.value;
          const appsLoadable = appsList.value;
          if (reposLoadable.status === 'not-loaded') void loadRepositories();
          if (appsLoadable.status === 'not-loaded') void loadApps();
          if (reposLoadable.status === 'failed') {
            return (
              <div class="cc-repo-submenu cc-repo-submenu-error" data-tooltip={reposLoadable.error}>
                <span>›</span>
                <span class="error-text">Failed to load repositories</span>
              </div>
            );
          }
          if (reposLoadable.status !== 'loaded') return null;
          const repos = reposLoadable.data;
          // External repos = registered repos minus the Lucidos-source row,
          // which is the implicit default and gets its own (top) option.
          const externalRepos = repos.filter(r => r.name !== 'Lucidos');
          const apps = appsLoadable.status === 'loaded' ? appsLoadable.data : [];
          // Encode the discriminated Scope on the option `value` so the
          // Dropdown — which only knows string values — can round-trip the
          // selection. `lucidos` is the literal, repo:<uuid> is external,
          // app:<id> is app.
          const SCOPE_LUCIDOS = 'lucidos';
          const scopeToOptionValue = (s: Scope): string => {
            switch (s.kind) {
              case 'lucidos': return SCOPE_LUCIDOS;
              case 'external': return `repo:${s.repoId}`;
              case 'app': return `app:${s.appId}`;
            }
          };
          const parseOptionValue = (v: string): Scope => {
            if (v.startsWith('repo:')) return { kind: 'external', repoId: v.slice(5) };
            if (v.startsWith('app:')) return { kind: 'app', appId: v.slice(4) };
            return { kind: 'lucidos' };
          };
          const options: Array<{ value: string; label: string; disabled?: boolean }> = [
            { value: SCOPE_LUCIDOS, label: 'Lucidos' },
          ];
          if (externalRepos.length > 0) {
            options.push({ value: '__hdr-repos', label: 'External repos', disabled: true });
            for (const r of externalRepos) {
              options.push({ value: `repo:${r.id}`, label: r.name });
            }
          }
          if (apps.length > 0) {
            options.push({ value: '__hdr-apps', label: 'Apps', disabled: true });
            for (const a of apps) {
              options.push({ value: `app:${a.id}`, label: a.name });
            }
          }
          return (
            <div class="cc-repo-submenu">
              <span>›</span>
              <Dropdown
                options={options}
                value={scopeToOptionValue(selectedScope.value)}
                onChange={(v) => { selectedScope.value = parseOptionValue(v); }}
                class="cc-repo-selector"
              />
            </div>
          );
        })()}
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
            placeholder={focusedThreadId.value ? 'Post a follow up…' : 'Go ahead…'}
            rows={1}
            onInput={handleInput}
            onKeyDown={handleKeyDown}
            onPaste={handlePaste}
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
          {showCCCommands
            ? <CCControlMenu threadId={focusedThreadId.value ?? undefined} />
            : <TodoListIndicator />}
          {(() => {
            // WIP app preview toggle: visible only when the focused thread is
            // an app coding-agent thread AND that app is open in the
            // panel-overlay. Clicking flips the overlay iframe between live
            // workspace content and the worktree-served WIP via the engine's
            // `?thread_id=<id>` route (see api/apps.rs::serve_app_ui). The
            // toggle reverts to live when the user navigates away (cleared by
            // the focusedThreadId effect in actions/wipPreview.ts) or when
            // Apply removes the worktree (engine 404 → iframe onError reverts).
            const ft = focusedThread;
            if (!ft || ft.meta.codingAgentKind !== 'app') return null;
            const folder = ft.meta.codingAgentFolder;
            const appId = folder ? folder.split('/').filter(Boolean).pop() : undefined;
            const openApp = currentApp.value;
            if (!appId || !openApp || openApp.id !== appId) return null;
            const wipOn = wipPreviewThreadId.value === ft.meta.id;
            return (
              <button
                class={`icon-btn header-icon${wipOn ? ' active' : ''}`}
                data-tooltip={wipOn ? 'Showing the WIP app preview from this thread’s worktree. Click to return to the live app.' : 'Preview the in-flight changes from this app coding-agent thread in the panel.'}
                aria-pressed={wipOn}
                aria-label={wipOn ? 'Stop WIP app preview' : 'Show WIP app preview'}
                onClick={() => {
                  // pushNavState captures wipPreviewThreadId into the new
                  // entry — flip the signal first so the snapshot reflects
                  // the post-click state. Back/forward walks each toggle.
                  wipPreviewThreadId.value = wipOn ? null : ft.meta.id;
                  pushNavState();
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
              {attachMenuOpen.value && !isAnsweringQuestion && (
                <div class="image-attach-menu">
                  <button onClick={() => { attachMenuOpen.value = false; cameraOpen.value = true; }}>
                    <CameraIcon />
                    Camera
                  </button>
                  <button onClick={() => { attachMenuOpen.value = false; fileInputRef.current?.click(); }}>
                    <FileIcon />
                    File
                  </button>
                </div>
              )}
            </div>
          )}
          <div class={rightClass}>
            {isStacked ? (
              <>
                <div class="prompt-actions-subrow">
                  {liftSection ? sectionButtonNodes : null}
                  {slots.liftable}
                </div>
                <div class="prompt-actions-subrow">
                  {liftSection ? null : sectionButtonNodes}
                  {slots.primary}
                  {sendButton}
                  {submitMultiButton}
                </div>
              </>
            ) : (
              <>
                {sectionButtonNodes}
                {slots.liftable}
                {slots.primary}
                {sendButton}
                {submitMultiButton}
              </>
            )}
          </div>
        </div>
      </div>
      {cameraOpen.value && <CameraCapture />}
    </div>
  );
}
