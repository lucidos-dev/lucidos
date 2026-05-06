import { useRef, useEffect, useState } from 'preact/hooks';
import { signal } from '@preact/signals';
import { pendingChatMessage, showToast, inputMode, popupImageSrc, focusedThreadId, threadMap, repositories, selectedRepoId, panelUrl, panelTitle, showConfirm, cancelingThreadIds, effectiveThreadStatus } from '../../store/store';
import { sendMessage, loadRepositories } from '../../store/actions/chat';
import { handleSaveThread, handleUnsaveThread } from '../../store/actions/threads';
import { updateCompose, discardCompose, sendCompose, sendFollowup, ensureFocusedComposeThread, type ComposeMode } from '../../store/actions/compose';
import { getDraft } from '../../store/composeDrafts';
import { scrollToBottom, scrolledUp } from './scrollState';
import { CaptureIcon, ImageIcon, CameraIcon, FileIcon, CloseIcon, ClearIcon, GlobeIcon } from '../shared/icons';
import { Dropdown } from '../shared/Dropdown';
import { CCControlMenu, ccMenuOpenRequest } from './CCControlMenu';
import { WaitingBanner, getWaitingState } from './WaitingBanner';
import { focusIfNeeded, composeHandlers, isComposeFocusedHere } from './promptFocus';
import { syncTextareaValue, shouldSkipSyncWhileEditing } from './promptValueSync';
import { effectiveSendMode } from './promptToggleMode';
import { resizeTextarea, useFontMetricsResize } from './promptResize';
import { isMobile } from '../../utils/viewport';
import { errorDetail } from '../../utils/errorDetail';
import { extractPasteUrl, escapeMarkdownLinkText } from '../../utils/extractPasteUrl';
import { pastedImagesForCurrentThread, getPastedImages, removePastedImage } from './pastedImages';
import { attachImageToActiveDraft } from './attachToDraft';
import { computeCaptureGeometry, readDeviceAngle } from './cameraGeometry';

const attachMenuOpen = signal(false);
const cameraOpen = signal(false);

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

// Saved threads must always offer Unsave — once a saved thread auto-archives,
// canArchive flips to false, which would otherwise strand the user in the
// saved section with no exit. Save (the unsaved → save action) stays gated
// on canArchive so it doesn't appear in non-actionable states.
export function shouldShowSaveButton(isSaved: boolean, canArchive: boolean): boolean {
  return isSaved || canArchive;
}

function SaveThreadButton({ threadId }: { threadId: string }) {
  const saved = threadMap.value.get(threadId)?.meta.saved ?? false;
  const onClick = async () => {
    if (saved) {
      // Unsave is a deliberate gesture — confirm so a stray click doesn't
      // drop a saved thread out of its parking spot.
      if (await showConfirm('Remove this thread from the Saved section?', 'Remove')) {
        handleUnsaveThread(threadId);
      }
    } else {
      handleSaveThread(threadId);
    }
  };
  const cls = `action-btn save-thread-btn${saved ? '' : ' action-btn-confirm'}`;
  return (
    <button class={cls} onClick={onClick} aria-label={saved ? 'Unsave thread' : 'Save thread'}>
      {saved ? '✓ Saved' : 'Save'}
    </button>
  );
}

export function PromptInput() {
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
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
    const focusedHere = isComposeFocusedHere(tid ?? '');
    if (!shouldSkipSyncWhileEditing(el, sameThread, focusedHere)
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
    if (resizeTextarea(el) && !scrolledUp.value) {
      scrollToBottom();
    }
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      inputRef.current?.blur();
      return;
    }
    if (e.key === 'Enter' && !e.shiftKey && !isMobile()) {
      e.preventDefault();
      submit();
    }
  }

  async function submit() {
    const el = inputRef.current;
    if (!el) return;
    const msg = el.value.trim();
    const threadId = focusedThreadId.value;
    const currentImages = threadId ? getPastedImages(threadId) : [];
    if (!msg && currentImages.length === 0) return;
    el.value = '';
    el.style.height = 'auto';
    scrollToBottom();
    if (isMobile()) el.blur();

    const thread = threadId ? threadMap.value.get(threadId) : undefined;
    const useClaudeCode = effectiveSendMode(thread) === 'claude_code';

    if (threadId && thread?.meta.state === 'composing') {
      // Composing thread: send through compose so server transitions
      // state→active and clears compose fields atomically.
      sendCompose(threadId, { useClaudeCode }).catch((error) => {
        showToast('Failed to send message: ' + errorDetail(error), 'error');
      });
      return;
    }

    // Active thread follow-up (or no thread at all → backend will create one).
    const images = currentImages.length > 0 ? [...currentImages] : undefined;
    const sendPromise = threadId
      ? sendFollowup(threadId, msg, images, { useClaudeCode: useClaudeCode || undefined })
      : sendMessage(msg, images, { useClaudeCode: useClaudeCode || undefined });
    sendPromise.catch((error) => {
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
      updateCompose(id, { text: '', images: [] });
    } else {
      void discardCompose(id);
    }
    // Discard is an exit from compose — always drop focus so the mobile
    // keyboard goes down, regardless of which branch we took.
    el.blur();
  }

  function handleInput() {
    autoResize();
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

  function removeImage(index: number) {
    const id = focusedThreadId.value;
    if (!id) return;
    removePastedImage(id, index);
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
  const images = pastedImagesForCurrentThread.value;
  const hasContent = hasText || images.length > 0;
  // CC doesn't use browser context — hide the pill when it won't be sent
  const toggleMode = effectiveSendMode(focusedThread);
  const willUseClaudeCode = toggleMode === 'claude_code';
  const hasUrlContext = !!panelUrl.value && !willUseClaudeCode;
  const showCCCommands = willUseClaudeCode;

  const waitingState = getWaitingState();
  const canArchive = waitingState?.type === 'actions' && waitingState.actions.includes('archive');

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
    const status = effectiveThreadStatus(thread);
    if (status !== 'running' && status !== 'waiting_for_user_answer') {
      const next = new Set(cancelingThreadIds.value);
      next.delete(focused);
      cancelingThreadIds.value = next;
    }
  }, [focusedThreadId.value, threadMap.value, cancelingThreadIds.value]);

  return (
    <div class="prompt-input-container">
      {images.length > 0 && (
        <div key="images" class="image-preview-strip">
          {images.map((img, i) => (
            <div class="image-preview-item" key={i}>
              <img
                src={`data:${img.mimeType};base64,${img.base64}`}
                class="image-preview-thumb"
                onClick={() => { popupImageSrc.value = `data:${img.mimeType};base64,${img.base64}`; }}
              />
              <button class="icon-btn image-preview-remove" onClick={() => removeImage(i)} aria-label="Remove" data-tooltip="Remove"><CloseIcon /></button>
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
        <div class="prompt-actions-row">
          {showCCCommands && <CCControlMenu threadId={focusedThreadId.value ?? undefined} />}
          {isNarrow ? (
            <button
              class="icon-btn header-icon"
              onClick={() => fileInputRef.current?.click()}
              aria-label="Attach image"
            >
              <ImageIcon />
            </button>
          ) : (
            <div class="image-attach-anchor" ref={menuRef}>
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
          <div class="prompt-actions-right">
            {hasContent && (
              <button
                class="action-btn action-btn-danger"
                onClick={handleDiscard}
                aria-label="Discard draft"
              >
                Discard draft
              </button>
            )}
            {focusedThreadId.value && !hasContent && shouldShowSaveButton(focusedThread?.meta.saved ?? false, canArchive) && <SaveThreadButton threadId={focusedThreadId.value} />}
            {!hasContent && waitingState
              ? <WaitingBanner state={waitingState} />
              : (
                <button
                  class={`action-btn${hasContent ? '' : ' invisible'}`}
                  onClick={submit}
                  aria-label="Send message"
                >
                  Send
                </button>
              )}
          </div>
        </div>
      </div>
      {cameraOpen.value && <CameraCapture />}
    </div>
  );
}
