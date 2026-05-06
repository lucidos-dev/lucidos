import { useState, useRef, useEffect, useCallback } from 'preact/hooks';
import { ConnectionStatus } from './ConnectionStatus';
import { panelOverlay, panelUrl, splitRatio, threadDrawerOpen, threadSearchQuery, mobileView, recoveryProgress, draftsViewActive } from '../../store/store';
import { useHideOnScroll } from '../../hooks/useHideOnScroll';
import { ThreadToggleButton } from '../shared/ThreadToggleButton';
import { ComposeIcon, SearchIcon, FilterIcon, DraftsIcon } from '../shared/icons';
import { ThreadNav } from '../shared/ThreadNav';
import { SearchEverywhereButton } from '../shared/SearchEverywhereButton';
import { threadChannelFilter, ALL_CHANNELS } from '../../store/store';
import { unfocusThread } from '../../store/actions/threads';
import { openUrl } from '../../store/actions/artifacts';
import { navigateToPane, resolveSwipePane } from '../../store/actions/pane';
import { MobileAppHeader } from './MobileAppHeader';
import { SwipeTouch } from '../../utils/swipe';
import { PanelNav } from './PanelNav';
import { ContentHeaderActions } from './ContentHeaderActions';
import { ControlPanel, controlPanelOpen, controlPanelBadgeCount, controlPanelBadgeTooltip } from './ControlPanel';
import { ThreadFilterDropdown } from './ThreadFilterDropdown';
import { getContentTitle, getDiffDescription } from './headerHelpers';
import { resolveHeaderDblClick } from './headerDblClick';
import { createDblClickGate } from '../../utils/dblClickGate';
import { useThreadSearch } from '../../hooks/useThreadSearch';
import { isMobile } from '../../utils/viewport';
import { isTextInput } from '../../utils/dom';
import { tooltipWithShortcut } from '../../utils/shortcuts';

function ThreadsHeader() {
  const [filterOpen, setFilterOpen] = useState(false);
  const { searchOpen, searchInputRef, onSearchInput, onSearchKeyDown, closeSearch, openSearchHandlers } = useThreadSearch();
  const toggleRef = useRef<HTMLButtonElement>(null);
  const closeFilter = useCallback(() => setFilterOpen(false), []);
  const filterActive = threadChannelFilter.value.size < ALL_CHANNELS.length;

  return (
    <div class={`threads-header${searchOpen ? ' search-active' : ''}`}>
      <ThreadToggleButton />
      <div class="thread-search-bar">
        <svg class="thread-search-bar-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="7" cy="7" r="4.5" />
          <path d="M10.5 10.5L14 14" />
        </svg>
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
      <div style={{ position: 'relative' }}>
        <button
          ref={toggleRef}
          class={`icon-btn header-icon threads-header-btn${filterActive ? ' filter-active' : ''}`}
          onClick={() => setFilterOpen(!filterOpen)}
          disabled={draftsViewActive.value}
          aria-label="Filter threads"
          data-tooltip={draftsViewActive.value ? 'Filter unavailable in drafts view' : 'Filter threads'}
          style={draftsViewActive.value ? 'pointer-events: auto; cursor: default;' : undefined}
        >
          <FilterIcon />
        </button>
        {filterOpen && !draftsViewActive.value && <ThreadFilterDropdown onClose={closeFilter} toggleRef={toggleRef} />}
      </div>
      <span class="threads-header-title">Threads</span>
      <button
        class={`icon-btn header-icon threads-header-btn${draftsViewActive.value ? ' drafts-active' : ''}`}
        onClick={() => { draftsViewActive.value = !draftsViewActive.value; }}
        aria-label="Toggle drafts view"
        data-tooltip="Drafts"
      >
        <DraftsIcon />
      </button>
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
        <MobileAppHeader />

        {/* ─── Desktop: full header ─── */}
        <div class="desktop-header">
          <div class="thread-header-elements">
            {threadDrawerOpen.value && <ThreadsHeader />}
            {!threadDrawerOpen.value && (
              <div class="collapsed-thread-actions">
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
            )}
            <span class="pane-header-brand">
              <div class="thread-nav-group">
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
                onClick={() => { controlPanelOpen.value = !controlPanelOpen.value; }}
                style={{ cursor: 'pointer' }}
              >
                <span class="pane-header-title">lucidos</span>
                {badgeCount > 0 && <span class="badge brand-badge" data-tooltip={controlPanelBadgeTooltip()}>{badgeCount}</span>}
                <ConnectionStatus />
              </span>
              <ControlPanel />
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
          <div class="content-header-elements">
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
            <div class="pane-header-spacer" />
            <ContentHeaderActions />
          </div>
        </div>
      </header>
    </>
  );
}
