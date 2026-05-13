import { useRef, useEffect, useState } from 'preact/hooks';
import { signal } from '@preact/signals';
import { pendingChatMessage, showToast, inputMode, openImagePopupFromGroup, focusedThreadId, threadMap, repositories, selectedRepoId, panelUrl, panelTitle, cancelingThreadIds, effectiveThreadStatus, getThreadDisplaySection, isMidTurn } from '../../store/store';
import { sendMessage, loadRepositories, handleCancelExchange } from '../../store/actions/chat';
import { handleSaveThread, handleUnsaveThread, handleArchiveThread } from '../../store/actions/threads';
import { answerCCQuestion } from '../../store/actions/chat-claude-code';
import type { DisplaySection } from '../../generated/thread-lifecycle';
import { computeExchanges, findQuestionAnswer, type AnswerKind, type ThreadState } from '../../store/thread-events';
import {
  multiSelectedByToolUse,
  pendingAnswerByToolUse,
  getMultiSelectedIds,
  setMultiSelectedIds,
  setPendingAnswer,
  clearPendingAnswer,
} from './QuestionCard';
import { updateCompose, discardCompose, sendCompose, sendFollowup, ensureFocusedComposeThread, type ComposeMode } from '../../store/actions/compose';
import { getDraft } from '../../store/composeDrafts';
import { scrollToBottom, preserveAtBottom } from './scrollState';
import { CaptureIcon, ImageIcon, CameraIcon, FileIcon, CloseIcon, ClearIcon, GlobeIcon } from '../shared/icons';
import { Dropdown } from '../shared/Dropdown';
import { CCControlMenu, ccMenuOpenRequest } from './CCControlMenu';
import { getBannerSlots, getWaitingState, type BannerState } from './WaitingBanner';
import { useFitsInOneRow } from '../../hooks/useFitsInOneRow';
import { focusIfNeeded, composeHandlers } from './promptFocus';
import { syncTextareaValue, shouldSkipSyncWhileEditing } from './promptValueSync';
import { effectiveSendMode } from './promptToggleMode';
import { resizeTextarea, useFontMetricsResize } from './promptResize';
import { isMobile } from '../../utils/viewport';
import { errorDetail } from '../../utils/errorDetail';
import { extractPasteUrl, escapeMarkdownLinkText } from '../../utils/extractPasteUrl';
import { attachedImagesForCurrentThread, getAttachedImages, removeAttachedImage } from './pastedImages';
import { getPendingUploads, hasInFlightUploads, removePendingUpload, pendingUploads } from '../../store/pendingUploads';
import { attachImageToActiveDraft } from './attachToDraft';
import { computeCaptureGeometry, readDeviceAngle } from './cameraGeometry';

const attachMenuOpen = signal(false);
const cameraOpen = signal(false);
/** Thread IDs where Send was just clicked but the thread hasn't reached
 *  running/waiting_for_user_answer yet. Drives the optimistic Send→Cancel
 *  morph so the action slot doesn't flash empty during the request gap.
 *  Cleared when the thread becomes cancellable (via the effect below) or
 *  on send failure (via the catch handler in submit). */
export const submittingThreadIds = signal<Set<string>>(new Set());

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
export function composeHasContent(
  hasText: boolean,
  attachedImagesCount: number,
  pendingUploadsCount: number,
): boolean {
  return hasText || attachedImagesCount > 0 || pendingUploadsCount > 0;
}

// The button is always rendered EXCEPT in 'hidden' mode so Send↔Cancel keeps
// its color morph without a DOM swap; the leave path snap-unmounts like the
// sibling section buttons — no fade-out, no position:absolute jump.
//   send        — visible, blue, click=submit
//   cancel      — visible, red,  click=cancel exchange
//   canceling   — visible, red,  disabled, label "Cancel..."
//   placeholder — invisible (visibility:hidden, takes space) to keep row height
//   hidden      — not rendered; banner or section buttons own the slot
type MorphMode = 'send' | 'cancel' | 'canceling' | 'placeholder' | 'hidden';

export function computeMorphMode(args: {
  hasContent: boolean;
  cancelTargetId: string | null;
  isCanceling: boolean;
  hasBannerOrSectionButtons: boolean;
}): MorphMode {
  if (args.hasContent) return 'send';
  if (args.cancelTargetId !== null) return args.isCanceling ? 'canceling' : 'cancel';
  if (args.hasBannerOrSectionButtons) return 'hidden';
  return 'placeholder';
}

// Stamp cancelTargetId BEFORE invoking send. sendCompose's sync prefix
// clears the draft and flips state→'active' (section buttons appear); if
// cancelTargetId is still null at that render, morphMode resolves to
// 'hidden', the button unmounts, and Send→Cancel blinks instead of morphing.
// Raw new sends (threadId null) have no prior button to preserve and pick up
// the new id from focusedThreadId after send's sync prefix runs setFocusedThread.
export function dispatchSend(
  threadId: string | null,
  send: () => Promise<void>,
): { promise: Promise<void>; submittedId: string | null } {
  if (threadId) {
    const next = new Set(submittingThreadIds.value);
    next.add(threadId);
    submittingThreadIds.value = next;
  }
  const promise = send();
  const submittedId = threadId ?? focusedThreadId.value;
  if (!threadId && submittedId) {
    const next = new Set(submittingThreadIds.value);
    next.add(submittedId);
    submittingThreadIds.value = next;
  }
  return { promise, submittedId };
}

// Toggled options + the textarea's custom answer each count as one selection.
// Whitespace-only text is dropped to mirror submitMultiAnswer's text.trim().
export function computeSubmitMultiCount(toggledCount: number, customAnswerText: string): number {
  return toggledCount + (customAnswerText.trim().length > 0 ? 1 : 0);
}

/** Latest unanswered multi-select `UserQuestionAsked` on the thread — each
 *  pending question lives in its own divider exchange (the `UserQuestionAsked`
 *  is the exchange's `userEvent`). Callers must gate by status; this walks
 *  every exchange and is too expensive to run on every keystroke otherwise. */
export function findPendingMultiSelectQuestion(
  thread: ThreadState | undefined,
): { toolUseId: string } | null {
  if (!thread) return null;
  const exchanges = computeExchanges(thread);
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const ex = exchanges[i];
    const ue = ex.userEvent;
    if (ue.type !== 'UserQuestionAsked' || !ue.multi_select) continue;
    if (findQuestionAnswer(ex, ue.tool_use_id)) return null;
    return { toolUseId: ue.tool_use_id };
  }
  return null;
}

// Apply & Restart is the only case where the bottom sub-row
// [Save][Discard][Apply & Restart] still overflows a phone-width
// .prompt-actions-subrow (no flex-wrap) after Diff lifts. Lift Save too so
// [Discard][Apply & Restart] stays on a row that fits.
export function shouldLiftSectionButtons(
  isStacked: boolean,
  bannerState: BannerState | null,
): boolean {
  return isStacked
    && bannerState?.type === 'actions'
    && bannerState.requiresRestart;
}

// Archive on Review threads is rendered by WaitingBanner (via resolveActions);
// this fills in the rest. Render = WaitingBanner ∪ getPromptSectionButtons.
//
// Saved threads always carry the unsave toggle ("✓ Saved") so the user can
// drop the thread back to regular flow at any time. Archive sits next to it
// only when idle and not active — Send/Cancel takes the slot otherwise.
//
// Active = mid-turn OR has active children. While active, the action area
// collapses to a single save/unsave indicator: saved wins over active in
// display_section, so a non-saved active thread lands in `active` (Save) and
// a saved one in `saved` (✓ Saved). The review/archive arms here only matter
// for the rare race where status flips before display_section recomputes.
//
// Pending Apply: WaitingBanner owns Discard + Apply. Save / ✓ Saved still
// shows next to it so the user can park the thread without resolving the
// pending change first. Archive is suppressed — a thread with pending changes
// shouldn't be archived, that's what Discard is for.
//
// Composing (hasContent): Send/Discard owns the action slot. Saved keeps its
// unsave toggle so the user can drop a Saved draft without sending first.
export function getPromptSectionButtons(
  section: DisplaySection,
  isActive: boolean,
  hasPendingChanges: boolean,
  hasContent: boolean,
): Array<'save' | 'archive' | 'unsave'> {
  if (hasPendingChanges) return section === 'saved' ? ['unsave'] : ['save'];
  if (hasContent) return section === 'saved' ? ['unsave'] : [];
  switch (section) {
    case 'review': return isActive ? [] : ['save'];
    case 'archive': return isActive ? [] : ['save'];
    case 'saved': return isActive ? ['unsave'] : ['unsave', 'archive'];
    case 'active': return ['save'];
  }
}

function SavePromptButton({ threadId }: { threadId: string }) {
  return (
    <button
      class="action-btn action-btn-confirm save-thread-btn"
      onClick={() => handleSaveThread(threadId)}
      aria-label="Save thread"
      data-row-item
    >
      Save
    </button>
  );
}

function ArchiveSavedPromptButton({ threadId }: { threadId: string }) {
  return (
    <button
      class="action-btn"
      onClick={() => handleArchiveThread(threadId)}
      aria-label="Archive thread"
      data-row-item
    >
      Archive
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
      onClick={() => handleUnsaveThread(threadId)}
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
  // Watch for pending messages from other modules (e.g. new app modal)
  useEffect(() => {
    const msg = pendingChatMessage.value;
    if (msg) {
      pendingChatMessage.value = null;
      sendMessage(msg).catch((error) => {
        showToast('Failed to send message: ' + errorDetail(error), 'error');
      });
    }
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

  // Close menu on outside click
  useEffect(() => {
    if (!attachMenuOpen.value) return;
    function onClick(e: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        attachMenuOpen.value = false;
      }
    }
    document.addEventListener('click', onClick);
    return () => document.removeEventListener('click', onClick);
  }, [attachMenuOpen.value]);

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
      if (hasPendingMultiQ) submitMultiAnswer();
      else submit();
    }
  }

  async function submit() {
    const el = inputRef.current;
    if (!el) return;
    const msg = el.value.trim();
    const threadId = focusedThreadId.value;
    const currentImages = threadId ? getAttachedImages(threadId) : [];
    if (!msg && currentImages.length === 0) return;
    const thread = threadId ? threadMap.value.get(threadId) : undefined;
    el.value = '';
    el.style.height = 'auto';
    scrollToBottom();
    if (isMobile()) el.blur();

    const useClaudeCode = effectiveSendMode(thread) === 'claude_code';

    const imageHashes = currentImages.length > 0 ? currentImages.map((i) => i.hash) : undefined;

    const { promise: sendPromise, submittedId } = dispatchSend(threadId, () => {
      if (threadId && thread?.meta.state === 'composing') {
        // Composing thread: send through compose so server transitions
        // state→active and clears compose fields atomically.
        return sendCompose(threadId, { useClaudeCode });
      } else if (threadId) {
        return sendFollowup(threadId, msg, imageHashes, { useClaudeCode: useClaudeCode || undefined });
      } else {
        return sendMessage(msg, imageHashes, { useClaudeCode: useClaudeCode || undefined });
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

  function handleDiscard() {
    const el = inputRef.current;
    if (!el) return;
    const id = focusedThreadId.value;
    if (!id) return;
    el.value = '';
    el.style.height = 'auto';
    const thread = threadMap.value.get(id);
    if (thread?.meta.state === 'active') {
      // Active thread: clear the in-progress follow-up text + images, but
      // keep the user in the thread. Deleting it would 409 ("thread is
      // active — use archive instead"). Compose fields are persisted
      // server-side for active threads too (cross-device draft sync), so
      // emptying them flows through updateCompose.
      updateCompose(id, { text: '', image_hashes: [] });
    } else {
      void discardCompose(id);
    }
    // Discard is an exit from compose — always drop focus so the mobile
    // keyboard goes down, regardless of which branch we took.
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
    const items = e.clipboardData?.items;
    if (!items) return;

    for (let i = 0; i < items.length; i++) {
      const item = items[i];
      if (item.type.startsWith('image/')) {
        e.preventDefault();
        const file = item.getAsFile();
        if (!file) continue;
        addImageFile(file);
        return; // Only process first image item
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
  const rawPendingMultiQ = focusedStatus === 'waiting_for_user_answer'
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

  const sectionButtons =
    tid && focusedThread
      ? getPromptSectionButtons(
          getThreadDisplaySection(focusedThread),
          isMidTurn(effectiveThreadStatus(focusedThread)) || focusedThread.meta.activeChildrenCount > 0,
          focusedThread.meta.ccHasChanges,
          hasContent,
        )
      : [];
  const sectionButtonNodes = tid && sectionButtons.length > 0
    ? sectionButtons.map(name => {
        switch (name) {
          case 'save': return <SavePromptButton key="save" threadId={tid} />;
          case 'unsave': return <UnsaveSavedPromptButton key="unsave" threadId={tid} />;
          case 'archive': return <ArchiveSavedPromptButton key="archive" threadId={tid} />;
        }
      })
    : null;

  const morphMode = computeMorphMode({
    hasContent: morphHasContent,
    cancelTargetId,
    isCanceling,
    hasBannerOrSectionButtons: !!bannerState || sectionButtons.length > 0,
  });

  // Clear the optimistic canceling flag once the thread leaves the cancellable
  // states ('running' or 'waiting_for_user_answer'). The set survives component
  // re-renders by design (button lives in the always-visible prompt area) —
  // without explicit release the flag would stick across the next stream and
  // disable the button before it's pressed. Both states must be included or
  // the flag would be cleared on the optimistic update before the cancel-side
  // events even land (waiting_for_user_answer briefly transitions through
  // 'running' via UserQuestionAnswered).
  useEffect(() => {
    const focused = focusedThreadId.value;
    if (!focused || !cancelingThreadIds.value.has(focused)) return;
    const thread = threadMap.value.get(focused);
    if (!thread) return;
    if (!isMidTurn(effectiveThreadStatus(thread))) {
      const next = new Set(cancelingThreadIds.value);
      next.delete(focused);
      cancelingThreadIds.value = next;
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
    const ok = await answerCCQuestion(focused, pendingMultiQ.toolUseId, answer);
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
  const slots = bannerState
    ? getBannerSlots(bannerState)
    : { liftable: composeDiscardButton, primary: null };
  const stacked = !fitsInOneRow;
  const sendButton = morphMode !== 'hidden' ? (
    <button
      key="send-cancel-morph"
      class={
        'action-btn send-cancel-morph'
        + (morphMode === 'cancel' || morphMode === 'canceling' ? ' action-btn-danger' : '')
        + (morphMode === 'placeholder' ? ' morph-placeholder' : '')
      }
      onClick={
        morphMode === 'send' ? submit
        : morphMode === 'cancel' ? () => handleCancelExchange(cancelTargetId!)
        : undefined
      }
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
          if (reposLoadable.status === 'not-loaded') loadRepositories();
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
          if (repos.length === 0) return null;
          const options = repos.map(r => ({
            value: r.name === 'Lucidos' ? '' : r.id,
            label: r.name,
          }));
          return (
            <div class="cc-repo-submenu">
              <span>›</span>
              <Dropdown
                options={options}
                value={selectedRepoId.value}
                onChange={(v) => { selectedRepoId.value = v; }}
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
          {showCCCommands && <CCControlMenu threadId={focusedThreadId.value ?? undefined} />}
          {isNarrow ? (
            <button
              class="icon-btn header-icon"
              onClick={() => fileInputRef.current?.click()}
              aria-label="Attach image"
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
                data-tooltip="Attach image"
                aria-label="Attach image"
              >
                <ImageIcon />
              </button>
              {attachMenuOpen.value && (
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
