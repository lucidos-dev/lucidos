import { useRef, useEffect, useState } from 'preact/hooks';
import { signal } from '@preact/signals';
import { pendingChatMessage, showToast, inputMode, popupImageSrc, focusedThreadId, focusedDraftId, threadMap, repositories, selectedRepoId, panelUrl, panelTitle } from '../../store/store';
import { sendMessage, loadRepositories } from '../../store/actions/chat';
import { syncDraftEntry } from '../../store/actions/drafts';
import {
  loadDraftText as loadDraftTextStorage,
  saveDraftText as saveDraftTextStorage,
} from '../../utils/draftStorage';
import { scrollToBottom, scrolledUp } from './scrollState';
import { CaptureIcon, ImageIcon, CameraIcon, FileIcon, CloseIcon, ClearIcon, GlobeIcon } from '../shared/icons';
import { Dropdown } from '../shared/Dropdown';
import { CCControlMenu, ccMenuOpenRequest } from './CCControlMenu';
import { WaitingBanner, getWaitingState } from './WaitingBanner';
import { focusIfNeeded, composeHandlers } from './promptFocus';
import { resizeTextarea, useFontMetricsResize } from './promptResize';
import { isMobile } from '../../utils/viewport';
import { errorDetail } from '../../utils/errorDetail';
import {
  pastedImagesForCurrentThread,
  getPastedImages,
  removePastedImage,
  clearPastedImages,
  hydratePastedImages,
  migrateLegacyPastedImages,
} from './pastedImages';
import { attachImageToActiveDraft } from './attachToDraft';
import { computeCaptureGeometry, readDeviceAngle } from './cameraGeometry';

const attachMenuOpen = signal(false);
const cameraOpen = signal(false);

function currentDraftId(): string {
  return focusedThreadId.value ?? focusedDraftId.value;
}

function saveDraftText(id: string, text: string) {
  saveDraftTextStorage(id, text);
  syncDraftEntry(id);
}

/** Clear text + images for the draft. */
function clearDraft(id: string) {
  clearPastedImages(id);
  saveDraftText(id, '');
}

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

export function PromptInput() {
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [hasText, setHasText] = useState(false);
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

  // Save/restore per-draft text and focus input when the active draft id changes.
  // Tracks whichever id (thread or compose draft) the prompt is currently bound to.
  const did = currentDraftId();
  const prevDidRef = useRef<string | undefined>(undefined);
  useEffect(() => {
    const el = inputRef.current;
    if (!el) return;

    const prev = prevDidRef.current;

    // Migrate from old global draft key (one-time, on first mount)
    if (prev === undefined) {
      const oldDraft = localStorage.getItem('lucidos-draft');
      if (oldDraft) {
        saveDraftTextStorage(did, oldDraft);
        localStorage.removeItem('lucidos-draft');
        syncDraftEntry(did);
      }
      migrateLegacyPastedImages(did);
    }

    // offsetParent (not offsetWidth) — both SplitLayout and MobileSwipeContainer
    // render PromptInput, and on mobile the desktop copy uses display:none but
    // can briefly report non-zero offsetWidth during transitions.
    if (prev !== undefined && prev !== did && el.offsetParent !== null) {
      saveDraftText(prev, el.value);
    }

    const draftText = loadDraftTextStorage(did);
    el.value = draftText;
    setHasText(draftText.length > 0);
    hydratePastedImages(did);

    autoResize();
    requestAnimationFrame(() => requestAnimationFrame(() => autoResize()));

    prevDidRef.current = did;

    if (!isMobile()) {
      requestAnimationFrame(() => focusIfNeeded(el));
    }
  }, [did]);

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
    let msg = el.value.trim();
    const tid = focusedThreadId.value;
    const draftId = currentDraftId();
    const currentImages = getPastedImages(draftId);
    if (!msg && currentImages.length === 0) return;
    el.value = '';
    el.style.height = 'auto';
    setHasText(false);
    scrollToBottom();
    if (isMobile()) el.blur();

    const images = currentImages.length > 0 ? [...currentImages] : undefined;
    clearDraft(draftId);

    // Always send via sendMessage — the backend auto-injects into active
    // threads or starts a new exchange as appropriate. This avoids the
    // frontend needing to know thread state (which can be stale).
    const thread = tid ? threadMap.value.get(tid) : undefined;
    const isCCThread = thread?.meta.channel === 'claude_code';

    const useClaudeCode = isCCThread || (!tid && inputMode.value.type === 'claude_code');
    // Compose-mode sends carry the originating draft id so chat.ts can
    // promote it (clear storage + assign a fresh focusedDraftId) once the
    // server accepts the request. Thread follow-ups don't need this.
    const sendOpts = {
      useClaudeCode: useClaudeCode || undefined,
      composeDraftId: tid ? undefined : draftId,
    };
    sendMessage(msg, images, sendOpts).catch((error) => {
      showToast('Failed to send message: ' + errorDetail(error), 'error');
    });
  }

  function handleInput() {
    autoResize();
    const el = inputRef.current;
    if (!el) return;
    const val = el.value;
    setHasText(val.length > 0);
    // "/" prefix in CC mode opens command menu with filter
    const tid = focusedThreadId.value;
    const draftId = currentDraftId();
    const thread = tid ? threadMap.value.get(tid) : undefined;
    const isCCMode = thread?.meta.channel === 'claude_code' || (!tid && inputMode.value.type === 'claude_code');
    if (isCCMode && val.startsWith('/')) {
      el.value = '';
      saveDraftText(draftId, '');
      autoResize();
      setHasText(false);
      ccMenuOpenRequest.value = val.slice(1);
      return;
    }
    saveDraftText(draftId, val);
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
    const id = currentDraftId();
    removePastedImage(id, index);
    syncDraftEntry(id);
  }

  // Toggle visibility: always visible in compose view, fades out when a thread
  // is focused. Derived from showToggles so toggles mount immediately when
  // returning to compose — no useEffect sync needed.
  const showToggles = !focusedThreadId.value;
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
  // CC doesn't use browser context — hide the pill when it won't be sent
  const focusedThread = focusedThreadId.value ? threadMap.value.get(focusedThreadId.value) : undefined;
  const willUseClaudeCode = focusedThreadId.value
    ? focusedThread?.meta.channel === 'claude_code'
    : inputMode.value.type === 'claude_code';
  const hasUrlContext = !!panelUrl.value && !willUseClaudeCode;
  const showCCCommands = willUseClaudeCode;

  const mode = inputMode.value;

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
          class={`segmented-btn ${mode.type === 'do' ? 'active' : ''}`}
          {...composeHandlers(() => { inputMode.value = { type: 'do' }; })}
        >
          Manifest
        </button>
        <button
          class={`segmented-btn ${mode.type === 'claude_code' ? 'active' : ''}`}
          {...composeHandlers(() => { inputMode.value = { type: 'claude_code' }; })}
        >
          Claude
        </button>
      </div>
        {!togglesFading && mode.type === 'claude_code' && (() => {
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
            placeholder="Go ahead…"
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
              setHasText(false);
              saveDraftText(currentDraftId(), '');
              autoResize();
              el.focus();
            }}
          >
            <ClearIcon />
          </button>
        </div>
        <input
          ref={fileInputRef}
          type="file"
          accept="image/*"
          multiple
          style={{ display: 'none' }}
          onChange={handleFileSelect}
        />
        <div class="prompt-actions-row">
          {showCCCommands && <CCControlMenu threadId={focusedThreadId.value ?? undefined} />}
          <div class="image-attach-anchor" ref={menuRef}>
            <button
              class="icon-btn header-icon"
              onClick={() => {
                if (isNarrow) {
                  fileInputRef.current?.click();
                } else {
                  attachMenuOpen.value = !attachMenuOpen.value;
                }
              }}
              data-tooltip={isNarrow ? undefined : 'Attach image'}
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
          {(() => {
            const hasContent = hasText || images.length > 0;
            if (!hasContent) {
              const waitingState = getWaitingState();
              if (waitingState) return <WaitingBanner state={waitingState} />;
            }
            return (
              <button
                class={`action-btn prompt-submit-btn${hasContent ? '' : ' invisible'}`}
                {...composeHandlers(submit)}
                aria-label="Send message"
              >
                Send
              </button>
            );
          })()}
        </div>
      </div>
      {cameraOpen.value && <CameraCapture />}
    </div>
  );
}
