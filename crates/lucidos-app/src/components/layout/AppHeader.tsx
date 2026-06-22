import { useState, useRef, useEffect, useCallback } from 'preact/hooks';
import { ConnectionStatus } from './ConnectionStatus';
import { panelOverlay, panelUrl, splitRatio, threadDrawerOpen, threadSearchQuery, mobileView, recoveryProgress, drawerView, attentionThreadCount } from '../../store/store';
import { useHideOnScroll } from '../../hooks/useHideOnScroll';
import { ThreadToggleButton } from '../shared/ThreadToggleButton';
import { ComposeIcon, SearchIcon } from '../shared/icons';
import { ThreadNav } from '../shared/ThreadNav';
import { SearchEverywhereButton } from '../shared/SearchEverywhereButton';
import { unfocusThread } from '../../store/actions/threads';
import { openUrl } from '../../store/actions/artifacts';
import { navigateToPane, resolveSwipePane, focusPane } from '../../store/actions/pane';
import { MobileAppHeader } from './MobileAppHeader';
import { SwipeTouch } from '../../utils/swipe';
import { PanelNav } from './PanelNav';
import { ContentHeaderActions } from './ContentHeaderActions';
import { ControlPanel, controlPanelBadgeCount, controlPanelBadgeTooltip, toggleControlPanelAtClick } from './ControlPanel';
import { ThreadFilterDropdown, viewIcon } from './ThreadFilterDropdown';
import { getContentTitle, getDiffDescription } from './headerHelpers';
import { resolveHeaderDblClick } from './headerDblClick';
import { createDblClickGate } from '../../utils/dblClickGate';
import { useThreadsHeaderState } from '../../hooks/useThreadsHeaderState';
import { isMobile } from '../../utils/viewport';
import { isTextInput } from '../../utils/dom';
import { tooltipWithShortcut } from '../../store/actions/keybindings';

function ThreadsHeader() {
  const { filterOpen, setFilterOpen, toggleRef, closeFilter, filterActive,
          searchOpen, searchInputRef, onSearchInput, onSearchKeyDown, closeSearch, openSearchHandlers } = useThreadsHeaderState();

  // The unified Filter control is active when a non-default drawer view is
  // selected OR a channel filter is set. The needs-attention badge rides on the
  // same button (attention-only — review/running/drafts stay per-row counts in
  // the menu).
  const filterButtonActive = drawerView.value !== 'all' || filterActive;
  const attentionCount = attentionThreadCount.value;
  // The button glyph reflects the selected view (funnel for `all`).
  const ViewIcon = viewIcon(drawerView.value);

  return (
    <div class={`threads-header${searchOpen ? ' search-active' : ''}`}
         onPointerDown={() => focusPane('drawer')}>
      <div class="thread-search-bar">
        <SearchIcon className="thread-search-bar-icon" />
        <input
          ref={searchInputRef}
          class="thread-search-input"
          type="text"
          placeholder="Search threads..."
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
      <div class="view-selector-slot">
        <button
          ref={toggleRef}
          class={`icon-btn header-icon threads-header-btn${filterButtonActive ? ' view-selector-active' : ''}`}
          onClick={() => setFilterOpen(!filterOpen)}
          aria-label="Filter threads"
          aria-haspopup="menu"
          aria-expanded={filterOpen}
          data-tooltip="Filter threads"
        >
          <ViewIcon />
          {attentionCount > 0 && <span class="badge">{attentionCount}</span>}
        </button>
        {filterOpen && <ThreadFilterDropdown onClose={closeFilter} toggleRef={toggleRef} />}
      </div>
      <span class="threads-header-title">Threads</span>
      <button
        class="icon-btn header-icon threads-header-btn"
        {...openSearchHandlers}
        aria-label="Search threads"
        data-tooltip="Search threads"
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
  // Visible children of brand-label open the control panel; the brand-label
  // itself stretches with flex:1 to center them, so its empty space should
  // still toggle full-width on dblclick like any other header gap.
  const brandLabel = el.closest('[data-role="control-panel-toggle"]');
  if (brandLabel && el !== brandLabel) return true;
  return false;
}

const headerDblGate = createDblClickGate();

function onHeaderClick(e: MouseEvent) {
  if (!isInteractive(e.target as HTMLElement) && !isMobile()) headerDblGate.record();
}

function onHeaderDblClick(e: MouseEvent) {
  if (!headerDblGate.allow()) return;
  if (isInteractive(e.target as HTMLElement)) return;
  if (isMobile()) return;

  const header = (e.currentTarget as HTMLElement).getBoundingClientRect();
  const ratio = splitRatio.value;

  // Account for thread drawer offset + drawer divider when calculating the split point,
  // matching the CSS formula: divider-x = co + ddo + sr * (100% - co - ddo)
  const styles = getComputedStyle(document.documentElement);
  const co = parseFloat(styles.getPropertyValue('--content-offset') || '0');
  const ddo = threadDrawerOpen.value ? parseFloat(styles.getPropertyValue('--divider-width') || '0') : 0;

  const splitX = header.left + co + ddo + ratio * (header.width - co - ddo);

  const clickedThreadSide = e.clientX < splitX;

  resolveHeaderDblClick({ clickedThreadSide, ratio });
}

/** On mobile, let horizontal swipes on the header navigate between panes.
 *  The header sits above the app iframe (which captures touch events) and
 *  outside iOS Safari's edge gesture zone, making it the most reliable
 *  swipe target on mobile. No visual track feedback — the pane transition
 *  animates via CSS when mobileView changes. */
function useHeaderPaneSwipe(ref: { current: HTMLElement | null }): void {
  const touch = useRef(new SwipeTouch());

  const onTouchStart = useCallback((e: TouchEvent) => {
    if (!isMobile()) return;
    if (isTextInput(e.target as Element)) return;
    const t = e.touches[0];
    touch.current.start(t.clientX, t.clientY);
  }, []);

  const onTouchMove = useCallback((e: TouchEvent) => {
    const t = e.touches[0];
    if (touch.current.move(t.clientX, t.clientY) !== null) {
      e.preventDefault();
    }
  }, []);

  const onTouchEnd = useCallback(() => {
    const target = resolveSwipePane(touch.current.end(window.innerWidth));
    if (target) navigateToPane(target);
  }, []);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.addEventListener('touchstart', onTouchStart, { passive: true });
    el.addEventListener('touchmove', onTouchMove, { passive: false });
    el.addEventListener('touchend', onTouchEnd, { passive: true });
    el.addEventListener('touchcancel', onTouchEnd, { passive: true });
    return () => {
      el.removeEventListener('touchstart', onTouchStart);
      el.removeEventListener('touchmove', onTouchMove);
      el.removeEventListener('touchend', onTouchEnd);
      el.removeEventListener('touchcancel', onTouchEnd);
    };
  }, [onTouchStart, onTouchMove, onTouchEnd]);
}

export function AppHeader() {
  const badgeCount = controlPanelBadgeCount();
  const [editingUrl, setEditingUrl] = useState(false);
  const [urlDraft, setUrlDraft] = useState('');
  const urlInputRef = useRef<HTMLInputElement>(null);
  const headerRef = useRef<HTMLElement>(null);
  useHideOnScroll(headerRef);
  useHeaderPaneSwipe(headerRef);

  const url = panelUrl.value;
  const showUrlPreview = panelOverlay.value?.type === 'url-preview';

  const headerTitle = getContentTitle();
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
      <header ref={headerRef} class="pane-header app-header" data-mobile-view={mobileView.value} onClick={onHeaderClick} onDblClick={onHeaderDblClick}>
        {/* Subtle background wash behind the focused pane's header region.
            First child so DOM order keeps it under every header control (which
            carry positive or later-in-DOM z-index) while still sitting above
            the header's own background. Positioned + revealed entirely from CSS
            via :root[data-focused-pane] (shell.css); desktop-only. */}
        <div class="header-focus-bg" aria-hidden="true" />
        <MobileAppHeader />

        {/* ─── Desktop: full header ─── */}
        <div class="desktop-header">
          {/* Both nav-icon hosts stay mounted; CSS keyed on
              data-thread-drawer-open fades between them so the drawer
              toggle animates instead of popping (mount/unmount can't
              transition). */}
          <div class="thread-header-elements">
            <ThreadsHeader />
            <div class="collapsed-thread-actions" onPointerDown={() => focusPane('thread')}>
              <ThreadToggleButton />
              <ThreadNav showTooltip />
              <button
                class="icon-btn header-icon"
                onClick={() => unfocusThread()}
                aria-label="New thread"
                data-tooltip={tooltipWithShortcut('New thread', 'newThread')}
              >
                <ComposeIcon />
              </button>
            </div>
            <span class="pane-header-brand" onPointerDown={() => focusPane('thread')}>
              <div class="thread-nav-group">
                <ThreadToggleButton />
                <ThreadNav showTooltip />
                <button
                  class="icon-btn header-icon brand-compose-btn"
                  onClick={() => unfocusThread()}
                  aria-label="New thread"
                  data-tooltip={tooltipWithShortcut('New thread', 'newThread')}
                >
                  <ComposeIcon />
                </button>
              </div>
              <span
                class="pane-header-brand-label"
                data-role="control-panel-toggle"
                onClick={(e) => {
                  // brand-label stretches with flex:1 to center its content;
                  // ignore clicks on the empty space around the visible children.
                  if (e.target === e.currentTarget) return;
                  toggleControlPanelAtClick(e);
                }}
              >
                <span class="pane-header-title">Lucidos</span>
                {badgeCount > 0 && <span class="badge brand-badge" data-tooltip={controlPanelBadgeTooltip()}>{badgeCount}</span>}
                <ConnectionStatus />
              </span>
              <ControlPanel layout="desktop" />
              <SearchEverywhereButton showTooltip />
              {recoveryProgress.value && (
                <span
                  class="recovery-indicator"
                  data-tooltip={`Resuming sessions: ${recoveryProgress.value.completed}/${recoveryProgress.value.total}`}
                >
                  <svg class="recovery-spinner" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2">
                    <path d="M8 2a6 6 0 1 1-4.24 1.76" stroke-linecap="round" />
                  </svg>
                </span>
              )}
            </span>
          </div>
          <div class="content-header-elements" onPointerDown={() => focusPane('content')}>
            <PanelNav />
            <span class="pane-header-content-title">
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
                  <span class="pane-header-title-text" data-tooltip={diffDesc || headerTitle} data-tooltip-tap>{headerTitle}</span>
                )
              )}
            </span>
            <ContentHeaderActions />
          </div>
        </div>
      </header>
    </>
  );
}
