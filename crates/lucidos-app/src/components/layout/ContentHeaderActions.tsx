import { Fragment } from 'preact';
import type { ComponentChild } from 'preact';
import { useState, useEffect } from 'preact/hooks';
import { NotificationsBell } from '../notifications/NotificationsBell';
import { TimeTravelDropdown } from '../apps/TimeTravelDropdown';
import { activeMenuItem, panelOverlay, panelUrl, filePreviewSource, filePreviewEditing, appPseudoFullscreen, parseRepoPath } from '../../store/store';
import { closeUrl, refreshFilePreview } from '../../store/actions/artifacts';
import { getAppFrameSrc, getVisibleAppFrame, exitPseudoFullscreen, refreshAppUI } from '../../store/actions/apps';
import { CloseIcon, ReloadIcon, SearchIcon, PopOutIcon, FullscreenIcon, ExitFullscreenIcon, CodeIcon, EyeIcon, EditIcon } from '../shared/icons';
import { RENDERABLE_EXTS, isEditableDataFile } from '../files/previewExts';
import { isTauri, isIOSPwa } from '../../utils/platform';
import { webviewReload } from '../../utils/tauri';
import { openFileSearch } from '../files/fileSearchActions';
import { pushOverlay, removeOverlay } from '../../store/overlayStack';

/** Shared action buttons for the content side of the header (used by both mobile and desktop). */
export function ContentHeaderActions() {
  // Track native fullscreen state (standard + webkit-prefixed)
  const [isNativeFullscreen, setIsNativeFullscreen] = useState(false);
  useEffect(() => {
    const handler = () => {
      const doc = document as unknown as Record<string, unknown>;
      setIsNativeFullscreen(!!(doc.fullscreenElement || doc.webkitFullscreenElement));
    };
    document.addEventListener('fullscreenchange', handler);
    document.addEventListener('webkitfullscreenchange', handler);
    return () => {
      document.removeEventListener('fullscreenchange', handler);
      document.removeEventListener('webkitfullscreenchange', handler);
    };
  }, []);

  // Exit pseudo-fullscreen when navigating away from app view or on Escape key
  const overlay = panelOverlay.value;
  const isPseudo = appPseudoFullscreen.value;
  useEffect(() => {
    if (overlay?.type !== 'app-ui' && isPseudo) exitPseudoFullscreen();
  }, [overlay?.type, isPseudo]);

  // Pseudo-fullscreen is dismissable via the central Escape dispatcher: register
  // it on the overlay stack while active instead of hand-rolling a `document`
  // Escape listener (which would race the dispatcher).
  useEffect(() => {
    if (!isPseudo) return;
    pushOverlay({ id: 'pseudo-fullscreen', dismiss: exitPseudoFullscreen });
    return () => removeOverlay('pseudo-fullscreen');
  }, [isPseudo]);

  const isFullscreen = isNativeFullscreen || isPseudo;

  function toggleFullscreen() {
    const doc = document as unknown as Record<string, unknown>;

    // Exit native fullscreen
    if (doc.fullscreenElement || doc.webkitFullscreenElement) {
      if (typeof doc.exitFullscreen === 'function') {
        (doc.exitFullscreen as () => Promise<void>)().catch(() => { /* user is exiting; failure is benign */ });
      } else if (typeof doc.webkitExitFullscreen === 'function') {
        (doc.webkitExitFullscreen as () => void)();
      }
      return;
    }

    // Exit pseudo-fullscreen
    if (appPseudoFullscreen.value) {
      exitPseudoFullscreen();
      return;
    }

    // Try native fullscreen on the iframe, fall back to CSS pseudo-fullscreen
    const frame = getVisibleAppFrame();
    if (!frame) return;

    const anyFrame = frame as unknown as Record<string, unknown>;
    const request = (typeof anyFrame.requestFullscreen === 'function' && anyFrame.requestFullscreen.bind(frame))
      || (typeof anyFrame.webkitRequestFullscreen === 'function' && anyFrame.webkitRequestFullscreen.bind(frame));

    if (request) {
      (request() as Promise<void>).then(() => frame.focus()).catch(() => {
        // Native request failed (common on iOS) — use CSS fallback
        appPseudoFullscreen.value = true;
      });
    } else {
      // Fullscreen API completely unavailable — CSS fallback
      appPseudoFullscreen.value = true;
    }
  }

  // ── Build ordered action list — each key claimed exactly once ──
  const actions: Array<{ key: string; el: ComponentChild }> = [];
  const claimed = new Set<string>();

  function addAction(key: string, el: ComponentChild) {
    if (claimed.has(key)) throw new Error(`Header action "${key}" already claimed`);
    claimed.add(key);
    actions.push({ key, el });
  }

  function reloadButton(onClick: () => void, label: 'Refresh' | 'Reload' = 'Refresh', disabledTooltip?: string) {
    if (disabledTooltip) {
      // Override .icon-btn:disabled { pointer-events: none } so the tooltip
      // (which relies on hover events) can still explain why the button is off.
      return (
        <button class="icon-btn header-icon" disabled aria-label={disabledTooltip} data-tooltip={disabledTooltip} style="pointer-events: auto;">
          <ReloadIcon />
        </button>
      );
    }
    return (
      <button class="icon-btn header-icon" onClick={onClick} aria-label={label} data-tooltip={label}>
        <ReloadIcon />
      </button>
    );
  }

  // Context-specific actions — mutually exclusive via if/else
  if (overlay?.type === 'app-ui') {
    // preserveWip: the header refresh re-fetches whatever the iframe is
    // currently pointed at, including WIP. Apply landing on disk and direct
    // file-source edits still drop WIP — those paths call refreshAppUI()
    // with the default options.
    addAction('refresh', reloadButton(() => void refreshAppUI(undefined, { preserveWip: true })));
    addAction('time-travel', <TimeTravelDropdown />);
    // iOS standalone PWA cannot open same-origin links in an external browser
    // (all WebKit-based browsers on iOS share this limitation).
    if (!isIOSPwa()) {
      const appFrameSrc = getAppFrameSrc();
      addAction('open-in-tab',
        <a class="icon-btn header-icon app-open-in-tab" href={appFrameSrc ?? undefined} target="_blank" rel="noopener noreferrer" aria-label="Open in new tab" data-tooltip="Open in new tab">
          <PopOutIcon />
        </a>,
      );
    }
    addAction('fullscreen',
      <button class="icon-btn header-icon app-fullscreen" onClick={toggleFullscreen} aria-label={isFullscreen ? 'Exit fullscreen' : 'Fullscreen'} data-tooltip={isFullscreen ? 'Exit fullscreen' : 'Fullscreen'}>
        {isFullscreen ? <ExitFullscreenIcon /> : <FullscreenIcon />}
      </button>,
    );
  } else if (overlay?.type === 'url-preview') {
    addAction('reload', reloadButton(() => {
      if (isTauri()) { const url = panelUrl.value; if (url) webviewReload(url); }
    }, 'Reload'));
    addAction('close',
      <button class="icon-btn header-icon" onClick={closeUrl} aria-label="Close browser" data-tooltip="Close browser">
        <CloseIcon />
      </button>,
    );
  } else if (overlay?.type === 'file-preview') {
    const ext = overlay.path.split('.').pop()?.toLowerCase() || '';
    const hasRendered = RENDERABLE_EXTS.includes(ext);
    const repo = parseRepoPath(overlay.path);
    const isDiff = repo?.mode === 'diff';
    // Repo files are read at a git ref (not the live workspace), so they're not
    // inline-editable; only data files under a mutable prefix are.
    const editable = !repo && isEditableDataFile(overlay.path);
    const editing = filePreviewEditing.value && editable;

    // While editing, Save/Cancel live in the editor body (FilePreviewInline) and
    // refresh/source-toggle would fight the draft — so the header drops them and
    // keeps only the global actions.
    if (!editing) {
      addAction('refresh', reloadButton(
        refreshFilePreview,
        'Refresh',
        isDiff ? 'Diff is fixed to this change' : undefined,
      ));
      if (hasRendered) {
        const isSource = filePreviewSource.value;
        addAction('source-toggle',
          <button
            class="icon-btn header-icon"
            onClick={() => { filePreviewSource.value = !isSource; }}
            aria-label={isSource ? 'Show rendered' : 'Show source'}
            data-tooltip={isSource ? 'Show rendered' : 'Show source'}
          >
            {isSource ? <EyeIcon /> : <CodeIcon />}
          </button>,
        );
      }
      if (editable) {
        addAction('edit',
          <button
            class="icon-btn header-icon file-edit-btn"
            onClick={() => { filePreviewEditing.value = true; }}
            aria-label="Edit file"
            data-tooltip="Edit file"
          >
            <EditIcon />
          </button>,
        );
      }
    }
  } else if (!overlay && activeMenuItem.value === 'files') {
    addAction('search',
      <button class="icon-btn header-icon file-search-btn" onClick={(e) => openFileSearch(e.currentTarget)} aria-label="Search files" data-tooltip="Search files">
        <SearchIcon />
      </button>,
    );
  }

  // Global actions — always present
  addAction('notifications', <NotificationsBell />);

  return (
    <div class="content-header-actions">
      {actions.map(({ key, el }) => <Fragment key={key}>{el}</Fragment>)}
    </div>
  );
}
