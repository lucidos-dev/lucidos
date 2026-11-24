import { useState, useEffect, useLayoutEffect, useRef } from 'preact/hooks';
import { NotificationsBell } from '../notifications/NotificationsBell';
import { lucidos } from '@lucidos/sdk';
import { activeMenuItem, panelOverlay, panelUrl, filePreviewSource, filePreviewWrap, diffWholeFile, diffWholeFileEffective, diffSideBySide, filePreviewEditing, appPseudoFullscreen, parseRepoPath, appSearchOpen, repositories, workspacePath, type PanelOverlay } from '../../store/store';
import { loadedOr } from '../../store/types';
import { sideBySideDiffAvailable } from '../../store/diffBody';
import { wrapToggleAvailable } from '../../store/previewWrap';
import { closeUrl, openLocalFile, openUrlOutsideApp } from '../../store/actions/artifacts';
import { previewCopyPath, previewDiskPath } from '../../utils/previewPath';
import { copyToClipboard } from '../../utils/clipboard';
import { appPopoutHref, exitPseudoFullscreen, toggleAppSearch, popOutApp, toggleAppFullscreen } from '../../store/actions/apps';
import { findAvailable, findSurface, toggleFind } from '../../store/actions/find-bar';
import { panelRefreshAvailable, runPanelRefresh } from '../../store/panelRefresh';
import { DIFF_REFRESH_PINNED, panelRefreshLive } from './RefreshIndicator';
import { getRemPx } from '../../utils/dom';
import { nativeFullscreenElement } from '../../store/appFullscreenHost';
import { CloseIcon, ReloadIcon, SearchIcon, PopOutIcon, FullscreenIcon, ExitFullscreenIcon, CodeIcon, EyeIcon, EditIcon, FileIcon, DiffIcon, SideBySideColumnsIcon, WrapTextIcon, CopyIcon } from '../shared/icons';
import { RENDERABLE_EXTS, REPO_RENDERABLE_EXTS, isEditableDataFile } from '../files/previewExts';
import { isTauri, isIOSPwa, clipboardAbilities } from '../../utils/platform';
import { webviewReload } from '../../utils/tauri';
import { openFileSearch } from '../files/fileSearchActions';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { currentArtifactPreviewCapability, withPreviewCapability } from '../../store/actions/frame-capability';
import { pushOverlay, removeOverlay } from '../../store/overlayStack';
import { CollapsingActions, type HeaderActionSpec } from './headerActions';
import { useHeaderActionCollapse, type HeaderCollapseTargets } from '../../hooks/useHeaderActionCollapse';

const COLLAPSE_TARGETS: HeaderCollapseTargets = {
  container: '.content-header-elements',
  centre: '.pane-header-content-title',
  anchor: '.notifications-bell',
};

function popoutSpec(
  extraClass: string,
  label: string,
  sink: { href: string | null } | { onClick: () => void },
): HeaderActionSpec | null {
  if (isIOSPwa()) return null;
  return { key: 'open-in-tab', icon: () => <PopOutIcon />, extraClass, label, ...sink };
}

export function appPopoutAction(): HeaderActionSpec | null {
  return isTauri()
    ? popoutSpec('app-open-in-tab', 'Open in browser', { onClick: () => popOutApp() })
    : popoutSpec('app-open-in-tab', 'Open in new tab', { href: appPopoutHref() });
}

const OPEN_ON_DESKTOP = 'Open in default app';

function absoluteUrl(url: string): string {
  try {
    return new URL(url, location.href).href;
  } catch {
    return url;
  }
}

const OPENS_SANDBOXED = /\.(html?|svg|xhtml|xml)$/i;

export function filePreviewPopoutAction(encoded: string): HeaderActionSpec | null {
  const url = parseRepoPath(encoded) ? null : lucidos.data.url(encoded);
  if (!isTauri()) {
    if (url === null) return null;
    const href = OPENS_SANDBOXED.test(encoded) ? withPreviewCapability(url, currentArtifactPreviewCapability()) : url;
    return popoutSpec('file-open-in-tab', 'Open in new tab', { href });
  }
  const disk = previewDiskPath(encoded, workspacePath.value, loadedOr(repositories.value, []));
  if (disk) return popoutSpec('file-open-in-tab', OPEN_ON_DESKTOP, { onClick: () => openLocalFile(disk) });
  if (url) {
    return popoutSpec('file-open-in-tab', OPEN_ON_DESKTOP, { onClick: () => openUrlOutsideApp(absoluteUrl(url)) });
  }
  return null;
}

export function filePreviewCopyPathAction(encoded: string): HeaderActionSpec | null {
  if (!clipboardAbilities().copy) return null;
  return {
    key: 'copy-path',
    label: 'Copy path',
    icon: () => <CopyIcon />,
    onClick: () => {
      const { path, absolute } = previewCopyPath(encoded, workspacePath.value, loadedOr(repositories.value, []));
      copyToClipboard(path, absolute ? 'Path copied' : 'Relative path copied (the full path is not known yet)');
    },
  };
}

export function mobileRefreshActionShown(overlay: PanelOverlay, available: boolean): boolean {
  const predatesPull = overlay?.type === 'app-ui' || overlay?.type === 'file-preview';
  return predatesPull && panelRefreshLive(overlay, available);
}

function filePreviewOffers(path: string) {
  const ext = path.split('.').pop()?.toLowerCase() || '';
  const repo = parseRepoPath(path);
  const hasRendered = (repo ? REPO_RENDERABLE_EXTS : RENDERABLE_EXTS).includes(ext);
  const editable = !repo && isEditableDataFile(path);
  const editing = filePreviewEditing.value && editable;
  return { isDiff: repo?.mode === 'diff', hasRendered, editable, editing };
}

function openFilePreviewOffers(): ReturnType<typeof filePreviewOffers> | null {
  const overlay = panelOverlay.value;
  return overlay?.type === 'file-preview' ? filePreviewOffers(overlay.path) : null;
}

function reloadSpec(key: string, onClick: () => void, label: 'Refresh' | 'Reload' = 'Refresh', disabledTooltip?: string): HeaderActionSpec {
  return { key, label, icon: () => <ReloadIcon />, onClick, disabledTooltip };
}

function findSpec(): HeaderActionSpec {
  return {
    key: 'find',
    label: 'Find',
    tooltip: tooltipWithShortcut('Find', 'findInView'),
    icon: () => <SearchIcon />,
    onClick: () => toggleFind('content'),
    extraClass: 'find-btn',
    active: findSurface.value === 'content',
  };
}

export function filePreviewContentActions(path: string, mobile: boolean): HeaderActionSpec[] {
  const { isDiff, hasRendered, editable, editing } = filePreviewOffers(path);
  const actions: HeaderActionSpec[] = [];
  if (editing) return actions;

  if (isDiff && mobile) actions.push(reloadSpec('refresh', () => {}, 'Refresh', DIFF_REFRESH_PINNED));
  const popout = filePreviewPopoutAction(path);
  if (popout) actions.push(popout);
  const copyPath = filePreviewCopyPathAction(path);
  if (copyPath) actions.push(copyPath);
  if (findAvailable('content')) actions.push(findSpec());
  if (isDiff) {
    const wholeFile = diffWholeFileEffective.value;
    actions.push({
      key: 'diff-whole-file',
      label: wholeFile ? 'Show diff' : 'Show full file',
      icon: () => (wholeFile ? <DiffIcon /> : <FileIcon />),
      onClick: () => { diffWholeFile.value = !wholeFile; },
      extraClass: 'diff-whole-file-toggle',
    });
  }
  if (sideBySideDiffAvailable.value) {
    const sideBySideOn = diffSideBySide.value;
    actions.push({
      key: 'diff-side-by-side',
      label: sideBySideOn ? 'Show unified' : 'Show side by side',
      icon: () => (sideBySideOn ? <DiffIcon /> : <SideBySideColumnsIcon />),
      onClick: () => { diffSideBySide.value = !sideBySideOn; },
      extraClass: 'diff-side-by-side-toggle',
    });
  }
  if (hasRendered) {
    const isSource = filePreviewSource.value;
    const label = isSource ? 'Show rendered' : 'Show source';
    actions.push({
      key: 'source-toggle',
      label,
      tooltip: tooltipWithShortcut(label, 'toggleSourceView'),
      icon: () => (isSource ? <EyeIcon /> : <CodeIcon />),
      onClick: toggleSourceView,
    });
  }
  if (wrapToggleAvailable.value) {
    const wrapOn = filePreviewWrap.value;
    const label = wrapOn ? 'Stop wrapping long lines' : 'Wrap long lines';
    actions.push({
      key: 'wrap-toggle',
      label,
      tooltip: tooltipWithShortcut(label, 'toggleLineWrap'),
      icon: () => <WrapTextIcon />,
      onClick: toggleLineWrap,
      active: wrapOn,
      extraClass: 'file-preview-wrap-toggle',
    });
  }
  if (editable) {
    actions.push({
      key: 'edit',
      label: 'Edit file',
      icon: () => <EditIcon />,
      onClick: () => { filePreviewEditing.value = true; },
      extraClass: 'file-edit-btn',
    });
  }
  return actions;
}

export function toggleSourceView(): void {
  const offers = openFilePreviewOffers();
  if (offers && !offers.editing && offers.hasRendered) filePreviewSource.value = !filePreviewSource.value;
}

export function toggleLineWrap(): void {
  const offers = openFilePreviewOffers();
  if (offers && !offers.editing && wrapToggleAvailable.value) filePreviewWrap.value = !filePreviewWrap.value;
}

export function toggleAppFullscreenIfShown(): void {
  if (panelOverlay.value?.type === 'app-ui') toggleAppFullscreen();
}

interface Props {
  layout: 'desktop' | 'mobile';
}

export function ContentHeaderActions({ layout }: Props) {
  const hostRef = useRef<HTMLDivElement>(null);
  const [compact, setCompact] = useState(false);
  useLayoutEffect(() => {
    const row = hostRef.current?.closest<HTMLElement>('.content-header-elements');
    if (layout !== 'desktop' || !row) return;
    const measure = () => {
      const rem = parseFloat(getComputedStyle(row).getPropertyValue('--content-compact-breakpoint'));
      setCompact(row.clientWidth <= rem * getRemPx());
    };
    const observer = new ResizeObserver(measure);
    observer.observe(row);
    if (hostRef.current) observer.observe(hostRef.current);
    const title = row.querySelector('.pane-header-content-title');
    if (title) observer.observe(title);
    measure();
    return () => observer.disconnect();
  }, [layout]);
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

  const overlay = panelOverlay.value;
  const isPseudo = appPseudoFullscreen.value;
  useEffect(() => {
    if (overlay?.type !== 'app-ui' && isPseudo) exitPseudoFullscreen();
  }, [overlay?.type, isPseudo]);

  useEffect(() => {
    if (!isPseudo) return;
    pushOverlay({ id: 'pseudo-fullscreen', dismiss: exitPseudoFullscreen, hasPanel: false });
    return () => removeOverlay('pseudo-fullscreen');
  }, [isPseudo]);

  const isFullscreen = isNativeFullscreen || isPseudo;

  const actions: HeaderActionSpec[] = [];
  const claimed = new Set<string>();

  function addAction(spec: HeaderActionSpec) {
    if (claimed.has(spec.key)) throw new Error(`Header action "${spec.key}" already claimed`);
    claimed.add(spec.key);
    actions.push(spec);
  }

  const mobile = layout === 'mobile';
  if ((mobile && mobileRefreshActionShown(overlay, panelRefreshAvailable.value))
      || (compact && panelRefreshLive(overlay, panelRefreshAvailable.value))) {
    addAction(reloadSpec('refresh', () => void runPanelRefresh()));
  }

  if (overlay?.type === 'app-ui') {
    const popout = appPopoutAction();
    if (popout) addAction(popout);
    addAction(findSpec());
    const label = isFullscreen ? 'Exit fullscreen' : 'Fullscreen';
    addAction({
      key: 'fullscreen',
      label,
      tooltip: tooltipWithShortcut(label, 'toggleAppFullscreen'),
      icon: () => (isFullscreen ? <ExitFullscreenIcon /> : <FullscreenIcon />),
      onClick: toggleAppFullscreen,
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
    for (const a of filePreviewContentActions(overlay.path, mobile || compact)) addAction(a);
  } else if (!overlay && activeMenuItem.value === 'files') {
    addAction({
      key: 'search',
      label: 'Search files',
      tooltip: tooltipWithShortcut('Search files', 'searchFiles'),
      icon: () => <SearchIcon />,
      onClick: (e) => openFileSearch(e.currentTarget as HTMLElement),
      extraClass: 'file-search-btn',
    });
  } else if (!overlay && (activeMenuItem.value === 'apps' || activeMenuItem.value === 'plugins')) {
    addAction({
      key: 'search',
      label: activeMenuItem.value === 'plugins' ? 'Search plugins' : 'Search apps',
      tooltip: tooltipWithShortcut(activeMenuItem.value === 'plugins' ? 'Search plugins' : 'Search apps', 'searchApps'),
      icon: () => <SearchIcon />,
      onClick: toggleAppSearch,
      extraClass: 'apps-search-btn',
      active: appSearchOpen.value,
    });
  }

  const collapsedCount = useHeaderActionCollapse(hostRef, actions.length, compact ? 'mobile' : layout, COLLAPSE_TARGETS);

  return (
    <div class="content-header-actions" data-compact={compact || undefined} ref={hostRef}>
      <CollapsingActions actions={actions} collapsed={compact ? actions.length : collapsedCount} moreClass="content-header-more">
        <NotificationsBell />
      </CollapsingActions>
    </div>
  );
}
