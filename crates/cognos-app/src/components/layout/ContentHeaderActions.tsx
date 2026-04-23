import { Fragment } from 'preact';
import type { ComponentChild } from 'preact';
import { useState, useEffect } from 'preact/hooks';
import { UploadIndicatorBar } from './Header';
import { NotificationsBell } from '../notifications/NotificationsBell';
import { TimeTravelDropdown } from '../apps/TimeTravelDropdown';
import { activeMenuItem, panelOverlay, panelUrl, filePreviewSource, appPseudoFullscreen } from '../../store/store';
import { closeUrl } from '../../store/actions/artifacts';
import { getAppFrameSrc, getVisibleAppFrame, exitPseudoFullscreen, refreshAppUI } from '../../store/actions/apps';
import { CloseIcon, ReloadIcon, SearchIcon, PopOutIcon, FullscreenIcon, ExitFullscreenIcon, CodeIcon, EyeIcon } from '../shared/icons';
import { RENDERABLE_EXTS } from '../files/FilePreviewInline';
import { isTauri, isIOSPwa } from '../../utils/platform';
import { webviewReload } from '../../utils/tauri';
import { openFileSearch } from '../files/FileSearchModal';

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

  useEffect(() => {
    if (!isPseudo) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') exitPseudoFullscreen(); };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [isPseudo]);

  const isFullscreen = isNativeFullscreen || isPseudo;

  function toggleFullscreen() {
    const doc = document as unknown as Record<string, unknown>;

    // Exit native fullscreen
    if (doc.fullscreenElement || doc.webkitFullscreenElement) {
      if (typeof doc.exitFullscreen === 'function') (doc.exitFullscreen as () => Promise<void>)();
      else if (typeof doc.webkitExitFullscreen === 'function') (doc.webkitExitFullscreen as () => void)();
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

  // Context-specific actions — mutually exclusive via if/else
  if (overlay?.type === 'app-ui') {
    addAction('refresh',
      <button class="icon-btn header-icon" onClick={() => refreshAppUI()} aria-label="Refresh" data-tooltip="Refresh">
        <ReloadIcon />
      </button>,
    );
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
    addAction('reload',
      <button
        class="icon-btn header-icon"
        onClick={() => { if (isTauri()) { const url = panelUrl.value; if (url) webviewReload(url); } }}
        aria-label="Reload"
        data-tooltip="Reload"
      >
        <ReloadIcon />
      </button>,
    );
    addAction('close',
      <button class="icon-btn header-icon" onClick={closeUrl} aria-label="Close browser" data-tooltip="Close browser">
        <CloseIcon />
      </button>,
    );
  } else if (overlay?.type === 'file-preview') {
    const ext = overlay.path.split('.').pop()?.toLowerCase() || '';
    const hasRendered = RENDERABLE_EXTS.includes(ext);
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
  } else if (!overlay && activeMenuItem.value === 'files') {
    addAction('search',
      <button class="icon-btn header-icon file-search-btn" onClick={openFileSearch} aria-label="Search files" data-tooltip="Search files">
        <SearchIcon />
      </button>,
    );
  }

  // Global actions — always present
  addAction('notifications', <NotificationsBell />);

  return (
    <div class="content-header-actions">
      <UploadIndicatorBar />
      {actions.map(({ key, el }) => <Fragment key={key}>{el}</Fragment>)}
    </div>
  );
}
