import { Fragment } from 'preact';
import type { ComponentChild } from 'preact';
import { useState, useEffect, useRef } from 'preact/hooks';
import { NotificationsBell } from '../notifications/NotificationsBell';
import { activeMenuItem, panelOverlay, panelUrl, filePreviewSource, diffWholeFile, diffWholeFileEffective, filePreviewEditing, appPseudoFullscreen, parseRepoPath, appSearchOpen } from '../../store/store';
import { closeUrl, refreshFilePreview } from '../../store/actions/artifacts';
import { getAppFrameSrc, getVisibleAppFrame, exitPseudoFullscreen, refreshAppUI, toggleAppSearch } from '../../store/actions/apps';
import { CloseIcon, ReloadIcon, SearchIcon, PopOutIcon, FullscreenIcon, ExitFullscreenIcon, CodeIcon, EyeIcon, EditIcon, FileIcon, DiffIcon } from '../shared/icons';
import { RENDERABLE_EXTS, REPO_RENDERABLE_EXTS, isEditableDataFile } from '../files/previewExts';
import { isTauri, isIOSPwa } from '../../utils/platform';
import { webviewReload } from '../../utils/tauri';
import { openFileSearch } from '../files/fileSearchActions';
import { pushOverlay, removeOverlay } from '../../store/overlayStack';
import { OverflowMenu, type OverflowMenuContext } from '../shared/OverflowMenu';
import { useHeaderActionCollapse } from '../../hooks/useHeaderActionCollapse';

/** One context action as DATA, so the same record renders either as a
 *  full-size header icon button or as a row inside the collapsed ⋯ overflow
 *  menu. `icon` is a thunk — the header and the menu each need their own
 *  vnode. The notifications bell is NOT one of these: it's the always-visible,
 *  never-overflowed anchor and renders separately. */
interface HeaderActionSpec {
  key: string;
  /** aria-label, tooltip, and the ⋯ menu row text. */
  label: string;
  icon: () => ComponentChild;
  onClick?: (e: MouseEvent) => void;
  /** Renders an `<a target="_blank">` instead of a button (open-in-tab). */
  href?: string | null;
  /** Extra class(es) on the header button, e.g. `app-fullscreen`. */
  extraClass?: string;
  /** Toggled-on state — adds `filter-active` (apps/plugins search). */
  active?: boolean;
  /** Disabled with an explanatory tooltip (diff-pinned refresh). */
  disabledTooltip?: string;
}

/** Full-size header rendering — markup identical to the pre-collapse version. */
function renderHeaderAction(a: HeaderActionSpec): ComponentChild {
  const cls = `icon-btn header-icon${a.extraClass ? ` ${a.extraClass}` : ''}${a.active ? ' filter-active' : ''}`;
  if (a.href !== undefined) {
    return (
      <a class={cls} href={a.href ?? undefined} target="_blank" rel="noopener noreferrer" aria-label={a.label} data-tooltip={a.label}>
        {a.icon()}
      </a>
    );
  }
  if (a.disabledTooltip) {
    // Override .icon-btn:disabled { pointer-events: none } so the tooltip
    // (which relies on hover events) can still explain why the button is off.
    return (
      <button class={cls} disabled aria-label={a.disabledTooltip} data-tooltip={a.disabledTooltip} style="pointer-events: auto;">
        {a.icon()}
      </button>
    );
  }
  return (
    <button class={cls} onClick={a.onClick} aria-label={a.label} data-tooltip={a.label}>
      {a.icon()}
    </button>
  );
}

/** Collapsed rendering — a ⋯ menu row with the same label + handler. `ctx.run`
 *  closes the menu before firing (links keep their native navigation — `run`
 *  doesn't preventDefault). */
function renderMenuAction(a: HeaderActionSpec, ctx: OverflowMenuContext): ComponentChild {
  if (a.href !== undefined) {
    return (
      <a key={a.key} class="thread-overflow-item" role="menuitem" href={a.href ?? undefined} target="_blank" rel="noopener noreferrer" onClick={ctx.run(() => {})}>
        {a.icon()}
        {a.label}
      </a>
    );
  }
  if (a.disabledTooltip) {
    // aria-disabled, NOT the disabled attribute: a disabled <button> can't take
    // focus, and when this row is the FIRST [role="menuitem"] (diff-pinned
    // refresh collapses first) OverflowMenu's keyboard-open would focus a
    // no-op target and strand the arrow-key roving outside the panel. An
    // aria-disabled row stays focusable/perceivable and simply has no onClick.
    return (
      <button key={a.key} type="button" class="thread-overflow-item" role="menuitem" aria-disabled="true" data-tooltip={a.disabledTooltip}>
        {a.icon()}
        {a.label}
      </button>
    );
  }
  return (
    <button key={a.key} type="button" class="thread-overflow-item" role="menuitem" onClick={(e: MouseEvent) => ctx.run(() => a.onClick?.(e))(e)}>
      {a.icon()}
      {a.label}
    </button>
  );
}

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
  const actions: HeaderActionSpec[] = [];
  const claimed = new Set<string>();

  function addAction(spec: HeaderActionSpec) {
    if (claimed.has(spec.key)) throw new Error(`Header action "${spec.key}" already claimed`);
    claimed.add(spec.key);
    actions.push(spec);
  }

  function reloadSpec(key: string, onClick: () => void, label: 'Refresh' | 'Reload' = 'Refresh', disabledTooltip?: string): HeaderActionSpec {
    return { key, label, icon: () => <ReloadIcon />, onClick, disabledTooltip };
  }

  // Context-specific actions — mutually exclusive via if/else
  if (overlay?.type === 'app-ui') {
    // preserveWip: the header refresh re-fetches whatever the iframe is
    // currently pointed at, including WIP. Apply landing on disk and direct
    // file-source edits still drop WIP — those paths call refreshAppUI()
    // with the default options.
    addAction(reloadSpec('refresh', () => void refreshAppUI(undefined, { preserveWip: true })));
    // iOS standalone PWA cannot open same-origin links in an external browser
    // (all WebKit-based browsers on iOS share this limitation).
    if (!isIOSPwa()) {
      addAction({
        key: 'open-in-tab',
        label: 'Open in new tab',
        icon: () => <PopOutIcon />,
        href: getAppFrameSrc(),
        extraClass: 'app-open-in-tab',
      });
    }
    addAction({
      key: 'fullscreen',
      label: isFullscreen ? 'Exit fullscreen' : 'Fullscreen',
      icon: () => (isFullscreen ? <ExitFullscreenIcon /> : <FullscreenIcon />),
      onClick: toggleFullscreen,
      extraClass: 'app-fullscreen',
    });
  } else if (overlay?.type === 'url-preview') {
    addAction(reloadSpec('reload', () => {
      if (isTauri()) { const url = panelUrl.value; if (url) webviewReload(url); }
    }, 'Reload'));
    addAction({
      key: 'close',
      label: 'Close browser',
      icon: () => <CloseIcon />,
      onClick: closeUrl,
    });
  } else if (overlay?.type === 'file-preview') {
    const ext = overlay.path.split('.').pop()?.toLowerCase() || '';
    const repo = parseRepoPath(overlay.path);
    const isDiff = repo?.mode === 'diff';
    // Repo HTML has no rendered view (it shows as source — see REPO_RENDERABLE_EXTS),
    // so the source/rendered toggle is suppressed for it.
    const hasRendered = (repo ? REPO_RENDERABLE_EXTS : RENDERABLE_EXTS).includes(ext);
    // Repo files are read at a git ref (not the live workspace), so they're not
    // inline-editable; only data files under a mutable prefix are.
    const editable = !repo && isEditableDataFile(overlay.path);
    const editing = filePreviewEditing.value && editable;

    // While editing, Save/Cancel live in the editor body (FilePreviewInline) and
    // refresh/source-toggle would fight the draft — so the header drops them and
    // keeps only the global actions.
    if (!editing) {
      addAction(reloadSpec(
        'refresh',
        refreshFilePreview,
        'Refresh',
        isDiff ? 'Diff is fixed to this change' : undefined,
      ));
      // Diff-only: toggle between the unified hunks and the whole file in its
      // merged end state. Orthogonal to the source/rendered toggle below. Read
      // the effective state (which carries the added-file default) so the icon
      // matches what's shown, and write the inverse as an explicit override.
      if (isDiff) {
        const wholeFile = diffWholeFileEffective.value;
        addAction({
          key: 'diff-whole-file',
          label: wholeFile ? 'Show diff' : 'Show full file',
          icon: () => (wholeFile ? <DiffIcon /> : <FileIcon />),
          onClick: () => { diffWholeFile.value = !wholeFile; },
          extraClass: 'diff-whole-file-toggle',
        });
      }
      if (hasRendered) {
        const isSource = filePreviewSource.value;
        addAction({
          key: 'source-toggle',
          label: isSource ? 'Show rendered' : 'Show source',
          icon: () => (isSource ? <EyeIcon /> : <CodeIcon />),
          onClick: () => { filePreviewSource.value = !isSource; },
        });
      }
      if (editable) {
        addAction({
          key: 'edit',
          label: 'Edit file',
          icon: () => <EditIcon />,
          onClick: () => { filePreviewEditing.value = true; },
          extraClass: 'file-edit-btn',
        });
      }
    }
  } else if (!overlay && activeMenuItem.value === 'files') {
    addAction({
      key: 'search',
      label: 'Search files',
      icon: () => <SearchIcon />,
      onClick: (e) => openFileSearch(e.currentTarget as HTMLElement),
      extraClass: 'file-search-btn',
    });
  } else if (!overlay && (activeMenuItem.value === 'apps' || activeMenuItem.value === 'plugins')) {
    addAction({
      key: 'search',
      label: activeMenuItem.value === 'plugins' ? 'Search plugins' : 'Search apps',
      icon: () => <SearchIcon />,
      onClick: toggleAppSearch,
      extraClass: 'apps-search-btn',
      active: appSearchOpen.value,
    });
  }

  // ── Progressive collapse (desktop only — inert inside the mobile header) ──
  // When the 3-zone header row runs out of room the LEADING context actions
  // (nearest the title) move into a ⋯ overflow menu, two first and then one
  // more per step, until only ⋯ + the bell remain; the title starts
  // ellipsizing only after that. The ⋯ trigger sits nearest the title.
  const hostRef = useRef<HTMLDivElement>(null);
  const collapsedCount = useHeaderActionCollapse(hostRef, actions.length);
  const collapsed = actions.slice(0, collapsedCount);
  const visible = actions.slice(collapsedCount);

  return (
    <div class="content-header-actions" ref={hostRef}>
      {collapsed.length > 0 && (
        <OverflowMenu
          ariaLabel="More actions"
          extraClass="content-header-more"
          items={(ctx) => collapsed.map((a) => renderMenuAction(a, ctx))}
        />
      )}
      {visible.map((a) => <Fragment key={a.key}>{renderHeaderAction(a)}</Fragment>)}
      <NotificationsBell />
    </div>
  );
}
