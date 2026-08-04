import { useState, useRef, useEffect, useCallback } from 'preact/hooks';
import { ConnectionStatus } from './ConnectionStatus';
import { panelOverlay, panelUrl, splitRatio, threadDrawerOpen, threadSearchQuery, mobileView, recoveryProgress, drawerView, attentionThreadCount } from '../../store/store';
import { useHideOnScroll } from '../../hooks/useHideOnScroll';
import { useWindowDragRegion } from '../../hooks/useWindowDragRegion';
import { ThreadToggleButton } from '../shared/ThreadToggleButton';
import { ComposeIcon, SearchIcon } from '../shared/icons';
import { ThreadNav } from '../shared/ThreadNav';
import { SearchEverywhereButton } from '../shared/SearchEverywhereButton';
import { unfocusThread } from '../../store/actions/threads';
import { openUrl } from '../../store/actions/artifacts';
import { navigateToPane, resolveSwipePane, focusPane } from '../../store/actions/pane';
import { focusPromptNow } from '../chat/promptFocus';
import { MobileAppHeader } from './MobileAppHeader';
import { BackupReminderBanner } from './BackupReminderBanner';
import { SwipeTouch } from '../../utils/swipe';
import { PanelNav } from './PanelNav';
import { ContentHeaderActions } from './ContentHeaderActions';
import { ControlPanel, BrandBadge, toggleControlPanelAtClick } from './ControlPanel';
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

  // Header regions set the focused pane on `click`, NOT `pointerdown`: in the
  // Tauri build the whole header is a window-drag region (useWindowDragRegion),
  // and a press that turns into a window drag must not change the focused pane —
  // the user is moving the window, not picking a pane. A native window drag
  // suppresses the synthetic `click`, so firing focus on click means only a
  // real click (press + release, no drag) moves focus. Off Tauri this is just
  // "click to focus" and behaves the same as before. The other three regions
  // (collapsed-thread-actions, pane-header-brand, content-header-elements)
  // follow the same rule.
  return (
    <div class={`threads-header${searchOpen ? ' search-active' : ''}`}
         onClick={() => focusPane('drawer')}>
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

/** A click on the thread pane's header. It focuses the pane (the wash) AND puts
 *  the caret in the composer: the header band is the largest neutral surface on
 *  that side, and lighting the wash without moving DOM focus left the user
 *  typing into nothing. Clicks on a control are that control's (isInteractive),
 *  so they keep only the pane focus: Search Everywhere opens an overlay with its
 *  own input, the brand label opens the control panel, and the compose button
 *  focuses the prompt itself from inside the touch gesture. Desktop only, like
 *  the pane-focus half: on mobile this tap would raise the keyboard over the
 *  conversation the user is reading. */
function onThreadHeaderClick(e: MouseEvent) {
  focusPane('thread');
  if (isMobile() || isInteractive(e.target as HTMLElement)) return;
  focusPromptNow();
}

/** Window-drag gate (Tauri desktop): the whole header band drags the window,
 *  except presses on interactive controls. Module-level so it's a stable ref for
 *  useWindowDragRegion's effect deps (window-zoom stays on the strip, not here). */
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
  const [editingUrl, setEditingUrl] = useState(false);
  const [urlDraft, setUrlDraft] = useState('');
  const urlInputRef = useRef<HTMLInputElement>(null);
  const headerRef = useRef<HTMLElement>(null);
  useHideOnScroll(headerRef);
  useHeaderPaneSwipe(headerRef);
  // Docker-style: the whole header band drags the window (Tauri desktop). Window
  // zoom stays on the strip — the header's double-click keeps doing pane-maximize.
  useWindowDragRegion(headerRef, { canStart: headerCanDragStart });

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
        {/* Focused-pane wash: a faint lighter-blue tint over the focused pane's
            header segment (drawer / thread / content) — the visual cue for which
            pane is focused. One box per pane, each STATICALLY positioned over its
            own segment; a focus shift CROSSFADES (the outgoing pane's wash eases
            out, the incoming pane's eases in) rather than sliding. Each box rises
            past the header's top edge by --titlebar-inset so the focused pane is
            painted through the reclaimed macOS title-bar band to the very top of
            the window (0px, hence no-op, off the macOS Tauri build). First children
            so DOM order keeps them under every header control. Positioned +
            revealed entirely from CSS via :root[data-focused-pane] (shell.css);
            desktop-only. */}
        <div class="header-focus-wash" data-pane="drawer" aria-hidden="true" />
        <div class="header-focus-wash" data-pane="thread" aria-hidden="true" />
        <div class="header-focus-wash" data-pane="content" aria-hidden="true" />
        <MobileAppHeader />

        {/* ─── Desktop: full header ─── */}
        <div class="desktop-header">
          {/* Both nav-icon hosts stay mounted; CSS keyed on
              data-thread-drawer-open fades between them so the drawer
              toggle animates instead of popping (mount/unmount can't
              transition). */}
          <div class="thread-header-elements">
            <ThreadsHeader />
            {/* Focus on click, not pointerdown, so a window drag never shifts
                focus — see ThreadsHeader. */}
            <div class="collapsed-thread-actions" onClick={onThreadHeaderClick}>
              <ThreadToggleButton />
              <ThreadNav showTooltip />
            </div>
            {/* Focus on click, not pointerdown, so a window drag never shifts
                focus — see ThreadsHeader. */}
            <span class="pane-header-brand" onClick={onThreadHeaderClick}>
              <div class="thread-nav-group">
                <ThreadToggleButton />
                <ThreadNav showTooltip />
              </div>
              <span
                class="pane-header-brand-label"
                data-role="control-panel-toggle"
                onClick={(e) => {
                  // brand-label is absolutely centered on the pane; ignore clicks
                  // on the empty space around the visible children.
                  if (e.target === e.currentTarget) return;
                  toggleControlPanelAtClick(e);
                }}
              >
                <span class="pane-header-title">Lucidos</span>
                <BrandBadge />
                <ConnectionStatus />
              </span>
              {/* Right-side actions grouped so the centered brand-label can float
                  over the pane's true middle (space-between pins this cluster to
                  the right edge). Compose sits next to Search, mirroring the
                  mobile thread header. */}
              <span class="pane-header-brand-actions">
                <ControlPanel layout="desktop" />
                <button
                  class="icon-btn header-icon brand-compose-btn"
                  onClick={() => unfocusThread()}
                  aria-label="New thread"
                  data-tooltip={tooltipWithShortcut('New thread', 'newThread')}
                >
                  <ComposeIcon />
                </button>
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
            </span>
          </div>
          {/* Focus on click, not pointerdown, so a window drag never shifts
              focus — see ThreadsHeader. */}
          <div class="content-header-elements" onClick={() => focusPane('content')}>
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

        {/* Mobile's backup reminder lives INSIDE the header, because the mobile
            header is position:fixed and a sibling in the shell's flow would sit
            behind it. As a header child it rides along for free: useHideOnScroll
            observes this element's border box, so --mobile-header-height grows to
            include the bar, and with it the mobile --app-header-bottom and every
            pane's ::before spacer. It also hides and returns with the header on
            scroll, which is right for chrome. The desktop copy is a flow sibling
            in App.tsx; each renders only under its own viewport. */}
        <BackupReminderBanner layout="mobile" />
      </header>
    </>
  );
}
