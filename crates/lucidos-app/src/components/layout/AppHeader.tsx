import { useState, useRef, useEffect, useCallback } from 'preact/hooks';
import { panelOverlay, panelUrl, threadSearchQuery, mobileView } from '../../store/store';
import { useHideOnScroll } from '../../hooks/useHideOnScroll';
import { useWindowDragRegion } from '../../hooks/useWindowDragRegion';
import { watchTitlebarBand } from '../../store/actions/trafficLights';
import { ThreadToggleButton } from '../shared/ThreadToggleButton';
import { SearchIcon } from '../shared/icons';
import { SearchField } from '../shared/SearchField';
import { ThreadBackButton, ThreadForwardButton } from '../shared/ThreadNav';
import { openUrl } from '../../store/actions/artifacts';
import { swipeToNeighbourPane, focusPane } from '../../store/actions/pane';
import { focusPromptNow } from '../chat/promptFocus';
import { MobileAppHeader } from './MobileAppHeader';
import { BackupReminderBanner } from './BackupReminderBanner';
import { ConnectionBanner } from './ConnectionBanner';
import { IngressBanner } from './IngressBanner';
import { WebhookRefusalBanner } from './WebhookRefusalBanner';
import { AgentSandboxBanner } from './AgentSandboxBanner';
import { PeerClientBanner } from './PeerClientBanner';
import { SlownessBanner } from './SlownessBanner';
import { PaneDrag, attachPaneDragTouches, type PaneTrack } from './paneDrag';
import { HamburgerButton, ContentBackButton, ContentForwardButton } from './ContentNav';
import { ContentHeaderActions } from './ContentHeaderActions';
import { drawnDrawerShown, drawnSplitRatio } from './drawnLayout';
import { ContentRefreshButton } from './RefreshIndicator';
import { ThreadHeaderActions, ThreadHeaderHomeButton, ThreadHeaderNewThreadButton } from './ThreadHeaderActions';
import { BrandMenuButton } from './HeaderMark';
import { WorkspaceNameLabel } from './WorkspaceNameLabel';
import { getContentTitle, getContentTitleShort, getDiffDescription } from './headerHelpers';
import { headerDblClickRegion, resolveHeaderDblClick } from './headerDblClick';
import { createDblClickGate } from '../../utils/dblClickGate';
import { useThreadSearch } from '../../hooks/useThreadSearch';
import { ThreadFilterButton, ThreadGroupingButton, ThreadsPaneTitle } from './ThreadsHeaderControls';
import { isMobile } from '../../utils/viewport';
import { isTextInput } from '../../utils/dom';
import { tooltipWithShortcut } from '../../store/actions/keybindings';

function ThreadsHeader() {
  const { searchOpen, searchInputRef, onSearchInput, onSearchKeyDown, closeSearch, openSearchHandlers } = useThreadSearch();

  return (
    <div class={`threads-header${searchOpen ? ' search-active' : ''}`}
         onClick={() => focusPane('drawer')}>
      <div class="thread-search-bar">
        <SearchField
          inputRef={searchInputRef}
          inputClass="thread-search-input"
          placeholder="Search threads…"
          value={threadSearchQuery.value}
          onInput={onSearchInput}
          onKeyDown={onSearchKeyDown}
        />
        <button class="icon-btn header-icon thread-search-close" onClick={closeSearch} aria-label="Close search">
          <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round">
            <path d="M4 4l8 8M12 4l-8 8" />
          </svg>
        </button>
      </div>
      <ThreadFilterButton class="threads-header-btn" tooltip={tooltipWithShortcut('Filter threads', 'toggleThreadFilter')} />
      <ThreadsPaneTitle class="threads-header-title" />
      <ThreadGroupingButton class="threads-header-btn" />
      <button
        class="icon-btn header-icon threads-header-btn"
        {...openSearchHandlers}
        aria-label="Search threads"
        data-tooltip={tooltipWithShortcut('Search threads', 'searchThreads')}
      >
        <SearchIcon />
      </button>
    </div>
  );
}

function isInteractive(el: HTMLElement): boolean {
  const tag = el.tagName;
  if (tag === 'BUTTON' || tag === 'A' || tag === 'INPUT' || tag === 'SELECT') return true;
  if (el.closest('button, a, input, select, .hamburger-panel, .thread-toggle')) return true;
  return false;
}

function onThreadHeaderClick(e: MouseEvent) {
  focusPane('thread');
  if (isMobile() || isInteractive(e.target as HTMLElement)) return;
  focusPromptNow();
}

const headerCanDragStart = (target: HTMLElement) => !isInteractive(target);

const headerDblGate = createDblClickGate();

function onHeaderClick(e: MouseEvent) {
  if (!isInteractive(e.target as HTMLElement) && !isMobile()) headerDblGate.record();
}

function onHeaderDblClick(e: MouseEvent) {
  if (!headerDblGate.allow()) return;
  if (isInteractive(e.target as HTMLElement)) return;
  if (isMobile()) return;

  const header = (e.currentTarget as HTMLElement).getBoundingClientRect();
  const ratio = drawnSplitRatio.value;

  const styles = getComputedStyle(document.documentElement);
  const drawerWidthPx = parseFloat(styles.getPropertyValue('--content-offset') || '0');
  const drawerDividerPx = drawnDrawerShown.peek()
    ? parseFloat(styles.getPropertyValue('--divider-width') || '0')
    : 0;

  const region = headerDblClickRegion({
    x: e.clientX,
    headerLeft: header.left,
    headerWidth: header.width,
    drawerWidthPx,
    drawerDividerPx,
    ratio,
  });

  resolveHeaderDblClick({ region, ratio });
}

const HEADER_TRACK: PaneTrack = {
  paneWidth: () => window.innerWidth,
  grab: () => {},
  follow: () => {},
  navigate: swipeToNeighbourPane,
  rest: () => {},
};

function useHeaderPaneSwipe(ref: { current: HTMLElement | null }): void {
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const drag = new PaneDrag(HEADER_TRACK, { frameMayLead: () => false, mayCommit: () => true });
    return attachPaneDragTouches(el, drag, {
      gate: (e) => isMobile() && !isTextInput(e.target as Element),
    });
  }, []);
}

export function AppHeader() {
  const [editingUrl, setEditingUrl] = useState(false);
  const [urlDraft, setUrlDraft] = useState('');
  const urlInputRef = useRef<HTMLInputElement>(null);
  const headerRef = useRef<HTMLElement>(null);
  useHideOnScroll(headerRef);
  useHeaderPaneSwipe(headerRef);
  useWindowDragRegion(headerRef, { canStart: headerCanDragStart });
  useEffect(() => watchTitlebarBand(), []);

  const url = panelUrl.value;
  const showUrlPreview = panelOverlay.value?.type === 'url-preview';

  const headerTitle = getContentTitleShort();
  const headerTitleFull = getContentTitle();
  const diffDesc = getDiffDescription();
  const showContentTitle = !!headerTitle;

  const startEditingUrl = useCallback(() => {
    if (!showUrlPreview || !url) return;
    setUrlDraft(url);
    setEditingUrl(true);
  }, [showUrlPreview, url]);

  const cancelEditingUrl = useCallback(() => {
    setEditingUrl(false);
  }, []);

  const submitUrl = useCallback(() => {
    setEditingUrl(false);
    let finalUrl = urlDraft.trim();
    if (!finalUrl) return;
    if (!/^https?:\/\//i.test(finalUrl)) finalUrl = 'https://' + finalUrl;
    openUrl(finalUrl);
  }, [urlDraft]);

  useEffect(() => {
    if (editingUrl && urlInputRef.current) {
      urlInputRef.current.focus();
      urlInputRef.current.select();
    }
  }, [editingUrl]);

  useEffect(() => {
    if (!showUrlPreview) setEditingUrl(false);
  }, [showUrlPreview]);

  return (
    <>
      <header ref={headerRef} class="pane-header app-header" data-titlebar-band data-mobile-view={mobileView.value} onClick={onHeaderClick} onDblClick={onHeaderDblClick}>
        <div class="header-focus-wash" data-pane="drawer" aria-hidden="true" />
        <div class="header-focus-wash" data-pane="thread" aria-hidden="true" />
        <div class="header-focus-wash" data-pane="content" aria-hidden="true" />
        <MobileAppHeader />

        <div class="desktop-header">
          <div class="thread-header-elements">
            <div class="thread-toggle-slot">
              <ThreadToggleButton />
            </div>
            <ThreadsHeader />
            <span class="pane-header-brand" onClick={onThreadHeaderClick}>
              <ThreadHeaderHomeButton />
              <ThreadHeaderNewThreadButton />
              <span class="pane-header-brand-label">
                <ThreadBackButton showTooltip />
                <span class="pane-header-brand-center">
                  <BrandMenuButton placement="brand" />
                  <WorkspaceNameLabel />
                </span>
                <ThreadForwardButton showTooltip />
              </span>
              <ThreadHeaderActions />
            </span>
          </div>
          <div class="content-header-elements" onClick={() => focusPane('content')}>
            <HamburgerButton />
            <ContentRefreshButton />
            <span class="pane-header-content-title">
              <ContentBackButton />
              {showContentTitle && (
                showUrlPreview && editingUrl ? (
                  <input
                    ref={urlInputRef}
                    class="panel-url-input"
                    type="text"
                    value={urlDraft}
                    onInput={(e) => setUrlDraft((e.target as HTMLInputElement).value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') submitUrl();
                      if (e.key === 'Escape') cancelEditingUrl();
                    }}
                    onBlur={cancelEditingUrl}
                  />
                ) : showUrlPreview ? (
                  <span
                    class="panel-url-title"
                    onClick={startEditingUrl}
                    data-tooltip={url!}
                  >
                    {headerTitle}
                  </span>
                ) : (
                  <span class="pane-header-title-text" data-tooltip={diffDesc || headerTitleFull} data-tooltip-tap>{headerTitle}</span>
                )
              )}
              <ContentForwardButton />
            </span>
            <ContentHeaderActions layout="desktop" />
          </div>
        </div>

        <ConnectionBanner layout="mobile" />
        <SlownessBanner layout="mobile" />
        <IngressBanner layout="mobile" />
        <WebhookRefusalBanner layout="mobile" />
        <AgentSandboxBanner layout="mobile" />
        <PeerClientBanner layout="mobile" />
        <BackupReminderBanner layout="mobile" />
      </header>
    </>
  );
}
