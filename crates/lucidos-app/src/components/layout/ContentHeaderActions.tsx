import { useState, useEffect, useRef } from 'preact/hooks';
import { NotificationsBell } from '../notifications/NotificationsBell';
import { activeMenuItem, panelOverlay, panelUrl, filePreviewSource, diffWholeFile, diffWholeFileEffective, diffSideBySide, filePreviewEditing, appPseudoFullscreen, parseRepoPath, appSearchOpen } from '../../store/store';
import { sideBySideDiffAvailable } from '../../store/diffBody';
import { closeUrl, refreshFilePreview } from '../../store/actions/artifacts';
import { getAppFrameSrc, getVisibleAppFrame, getVisibleAppPanel, exitAppFullscreen, exitPseudoFullscreen, refreshAppUI, toggleAppSearch, popOutApp } from '../../store/actions/apps';
import { nativeFullscreenElement } from '../../store/appFullscreenHost';
import { CloseIcon, ReloadIcon, SearchIcon, PopOutIcon, FullscreenIcon, ExitFullscreenIcon, CodeIcon, EyeIcon, EditIcon, FileIcon, DiffIcon, SideBySideColumnsIcon } from '../shared/icons';
import { RENDERABLE_EXTS, REPO_RENDERABLE_EXTS, isEditableDataFile } from '../files/previewExts';
import { isTauri, isIOSPwa } from '../../utils/platform';
import { webviewReload } from '../../utils/tauri';
import { openFileSearch } from '../files/fileSearchActions';
import { pushOverlay, removeOverlay } from '../../store/overlayStack';
import { CollapsingActions, type HeaderActionSpec } from './headerActions';
import { useHeaderActionCollapse, type HeaderCollapseTargets } from '../../hooks/useHeaderActionCollapse';

/** The boxes the content row's collapse is measured against: the row, and the
 *  title cluster centred on it. No leading entry, because there is nothing to
 *  measure there: the cluster is centred, so the room these actions get is half
 *  of what it leaves rather than the row's leftover, and the hamburger leading
 *  the row is inside that half by a box and a half. Stable identity so the
 *  collapse effect's deps do not re-fire every render. */
const COLLAPSE_TARGETS: HeaderCollapseTargets = {
  container: '.content-header-elements',
  centre: '.pane-header-content-title',
  anchor: '.notifications-bell',
};

/** The control that takes the open app out of the shell and into a top-level
 *  page of its own, or null where the platform cannot offer one.
 *
 *  Exported so the platform decision is testable without standing the header up:
 *  which of the two shapes is returned is the whole bug fix, and neither shape
 *  is observable from the rendered markup alone (a dead anchor and a live one
 *  look identical).
 *
 *  Three platforms, three answers. An installed iOS PWA gets nothing: it cannot
 *  open a same-origin link anywhere but its own inescapable in-app web view (a
 *  limitation every WebKit-based iOS browser shares). A browser gets a real
 *  anchor, so cmd-click, middle-click and "copy link address" all work. The
 *  packaged desktop client gets a button, because its WKWebView silently drops
 *  a `target="_blank"` navigation, and there are no tabs there to open one in:
 *  see `popOutApp`, which hands the URL to the OS opener instead. */
export function appPopoutAction(): HeaderActionSpec | null {
  if (isIOSPwa()) return null;
  const spec = {
    key: 'open-in-tab',
    icon: () => <PopOutIcon />,
    extraClass: 'app-open-in-tab',
  };
  return isTauri()
    ? { ...spec, label: 'Open in browser', onClick: () => popOutApp() }
    : { ...spec, label: 'Open in new tab', href: getAppFrameSrc() };
}

interface Props {
  /** Which header this copy belongs to. Both are mounted at once and only one
   *  is visible, and the two collapse completely differently (see the render
   *  below), so the copy has to say which it is rather than sniff the DOM for
   *  an enclosing mobile row. Same shape as `ContentPane`. */
  layout: 'desktop' | 'mobile';
}

/** Shared action buttons for the content side of the header (used by both mobile and desktop). */
export function ContentHeaderActions({ layout }: Props) {
  // Track native fullscreen state. `nativeFullscreenElement` is the one reader
  // of both spellings (store/appFullscreenHost.ts), shared with the overlay
  // layer so the header and the layer can never disagree about what is
  // fullscreen.
  const [isNativeFullscreen, setIsNativeFullscreen] = useState(false);
  useEffect(() => {
    const handler = () => setIsNativeFullscreen(nativeFullscreenElement() !== null);
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
    // Already fullscreen (native, or the CSS fallback): come back to the normal
    // layout. Shared with the navigation that has to reveal something other
    // than the app, so there is one definition of how to leave.
    if (exitAppFullscreen()) return;

    // Try native fullscreen on the app PANEL (not the iframe), fall back to CSS
    // pseudo-fullscreen. The panel is the target because a natively fullscreen
    // element is painted alone, and an iframe renders no DOM children: with the
    // iframe fullscreen the host had nowhere to put its own modals and toasts,
    // so an app's previewFile / confirm / prompt showed nothing. The panel can
    // hold them, and OverlayLayer portals them in.
    const frame = getVisibleAppFrame();
    if (!frame) return;

    // No panel to go fullscreen INSIDE means the host would have nowhere to
    // paint its overlays there, so take the CSS fallback rather than leaving the
    // button dead: pseudo-fullscreen is a stacking question, and
    // --z-app-fullscreen already puts the host's modals and toasts above it.
    const panel = getVisibleAppPanel();
    if (!panel) {
      appPseudoFullscreen.value = true;
      return;
    }

    const anyPanel = panel as unknown as Record<string, unknown>;
    const request = (typeof anyPanel.requestFullscreen === 'function' && anyPanel.requestFullscreen.bind(panel))
      || (typeof anyPanel.webkitRequestFullscreen === 'function' && anyPanel.webkitRequestFullscreen.bind(panel));

    if (request) {
      // `Promise.resolve` + try/catch, not a bare `.then`: the UNPREFIXED
      // request returns a promise, but `webkitRequestFullscreen` returns void,
      // so calling `.then` on the result throws a TypeError synchronously on a
      // prefixed-only engine, inside the click handler, taking the CSS fallback
      // in the catch down with it and leaving the button dead. (A prefixed
      // engine reports failure through `webkitfullscreenerror` instead, which
      // this does not observe; the user's next press still falls back.)
      //
      // Focus the frame, not the panel: fullscreen moves focus to the element
      // that requested it, and the app is what the user is about to type into.
      try {
        Promise.resolve(request() as Promise<void> | undefined)
          .then(() => frame.focus())
          .catch(() => {
            // Native request rejected (common on iOS), so use the CSS fallback.
            appPseudoFullscreen.value = true;
          });
      } catch {
        // Threw synchronously (a disallowed request): same fallback.
        appPseudoFullscreen.value = true;
      }
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
    const popout = appPopoutAction();
    if (popout) addAction(popout);
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
      // Side-by-side is a rendering of the HUNKS, so it is offered only when the
      // hunks are what's showing (not the whole merged file, not the rendered
      // markdown diff) and only where two columns fit. Both conditions are the
      // body's own (`diffBodyKind`, `diffFitsSideBySide`), so the control cannot
      // appear over a view it would do nothing to.
      if (sideBySideDiffAvailable.value) {
        const sideBySideOn = diffSideBySide.value;
        addAction({
          key: 'diff-side-by-side',
          label: sideBySideOn ? 'Show unified' : 'Show side by side',
          icon: () => (sideBySideOn ? <DiffIcon /> : <SideBySideColumnsIcon />),
          onClick: () => { diffSideBySide.value = !sideBySideOn; },
          extraClass: 'diff-side-by-side-toggle',
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

  // ── Collapse ──
  // Desktop collapses PROGRESSIVELY: as the pane narrows the leading context
  // actions (nearest the title) move into a ⋯ overflow menu, two first and then
  // one more per step, until only ⋯ + the bell remain.
  //
  // What it is giving way TO changed on 2026-08-13, even though the steps did
  // not. The title used to be the flex middle of the row, so folding an icon
  // handed it that width; it is a box centred on the row now, clearing a
  // constant reserve at each end (--content-side-reserve in panels/shell.css),
  // so folding frees the cluster's own room and nothing else. Which means the
  // fold only has to fire where that reserve stops holding: the clamp's
  // min-span arm, a Canvas pane at or near its floor, where the box's ends do
  // reach the clusters. Above it the reserve is sized for the widest cluster
  // this row can carry and the measurement finds everything fits.
  //
  // Mobile collapses EVERYTHING, at every width, bar the exception below. The
  // trailing cluster is therefore ⋯ + the bell, or the bell alone for a view
  // with no context actions. It predates the desktop row being centred and
  // answers the same
  // question differently. A phone's row cannot afford a reserve wide enough for
  // a cluster that grows with the action count. So it bounds the cluster
  // instead, which pins the chevrons to a fixed span agreeing with the thread
  // pane's.
  //
  // One exception, and it agrees with the desktop rule rather than departing
  // from it: a view carrying exactly ONE action keeps that action's own icon.
  // The ⋯ trigger would stand in the same box, so the cluster is two boxes
  // either way and the menu buys only a tap. See `mobileCollapseCount`.
  const hostRef = useRef<HTMLDivElement>(null);
  // Three or more context actions fold whole, at any width: see
  // `alwaysCollapseFrom`. Two still ride the row when there is room for them.
  const collapsedCount = useHeaderActionCollapse(hostRef, actions.length, layout, COLLAPSE_TARGETS, {
    alwaysCollapseFrom: 3,
  });

  return (
    <div class="content-header-actions" ref={hostRef}>
      <CollapsingActions actions={actions} collapsed={collapsedCount} moreClass="content-header-more">
        <NotificationsBell />
      </CollapsingActions>
    </div>
  );
}
