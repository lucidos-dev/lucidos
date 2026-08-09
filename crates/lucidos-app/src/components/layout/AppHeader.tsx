import { useState, useRef, useEffect, useCallback } from 'preact/hooks';
import { panelOverlay, panelUrl, splitRatio, threadDrawerOpen, threadSearchQuery, mobileView } from '../../store/store';
import { useHideOnScroll } from '../../hooks/useHideOnScroll';
import { useWindowDragRegion } from '../../hooks/useWindowDragRegion';
import { ThreadToggleButton } from '../shared/ThreadToggleButton';
import { SearchIcon } from '../shared/icons';
import { ThreadBackButton, ThreadForwardButton } from '../shared/ThreadNav';
import { openUrl } from '../../store/actions/artifacts';
import { navigateToPane, resolveSwipePane, focusPane } from '../../store/actions/pane';
import { focusPromptNow } from '../chat/promptFocus';
import { MobileAppHeader } from './MobileAppHeader';
import { BackupReminderBanner } from './BackupReminderBanner';
import { SwipeTouch } from '../../utils/swipe';
import { HamburgerButton, ContentBackButton, ContentForwardButton } from './ContentNav';
import { ContentHeaderActions } from './ContentHeaderActions';
import { ThreadHeaderActions } from './ThreadHeaderActions';
import { BrandMenuButton } from './HeaderMark';
import { WorkspaceNameLabel } from './WorkspaceNameLabel';
import { getContentTitle, getContentTitleShort, getDiffDescription } from './headerHelpers';
import { headerDblClickRegion, resolveHeaderDblClick } from './headerDblClick';
import { createDblClickGate } from '../../utils/dblClickGate';
import { useThreadsHeaderState } from '../../hooks/useThreadsHeaderState';
import { isMobile } from '../../utils/viewport';
import { isTextInput } from '../../utils/dom';

function ThreadsHeader() {
  // The Filter button's whole appearance (glyph, active highlight, and the
  // attention-only badge, which is 0 while the panel is open) comes from the
  // shared hook, since the mobile header renders the same control.
  const { filterOpen, toggleFilter, filterButtonActive, FilterButtonIcon, filterButtonBadge,
          searchOpen, searchInputRef, onSearchInput, onSearchKeyDown, closeSearch, openSearchHandlers } = useThreadsHeaderState();

  // Header regions set the focused pane on `click`, NOT `pointerdown`: in the
  // Tauri build the whole header is a window-drag region (useWindowDragRegion),
  // and a press that turns into a window drag must not change the focused pane —
  // the user is moving the window, not picking a pane. A native window drag
  // suppresses the synthetic `click`, so firing focus on click means only a
  // real click (press + release, no drag) moves focus. Off Tauri this is just
  // "click to focus" and behaves the same as before. The other three regions
  // (thread-toggle-slot, pane-header-brand, content-header-elements) follow the
  // same rule.
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
      {/* The button only toggles: the panel itself renders down in the drawer
          pane (see ThreadDrawer), so it is not `aria-haspopup` chrome anymore.
          It is also the panel's only way out, which is what the X glyph says
          while the panel is up (see useThreadsHeaderState). */}
      <div class="view-selector-slot">
        <button
          class={`icon-btn header-icon threads-header-btn${filterButtonActive ? ' view-selector-active' : ''}`}
          onClick={toggleFilter}
          aria-label="Filter threads"
          aria-expanded={filterOpen}
          data-tooltip="Filter threads"
        >
          <FilterButtonIcon />
          {filterButtonBadge > 0 && <span class="badge">{filterButtonBadge}</span>}
        </button>
      </div>
      {/* The pane's title says what the pane is showing: the list, or the filter
          panel that has taken it over (ThreadFilterPanel, which carries no title
          row of its own so this one is not repeated two rows down). Just
          "Filters": the pane is already the Threads pane, and the drawer is the
          only thing on screen the filter could be filtering. */}
      <span class="threads-header-title">{filterOpen ? 'Filters' : 'Threads'}</span>
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

/** The brand label used to need a carve-out here: it opened the Lucidos menu
 *  from any of its visible children, while its own empty centring space had to
 *  keep behaving like a header gap. The mark owns that job now and is a real
 *  `<button>`, which the `closest` line already covers, and the workspace name
 *  beside it is a plain label, so the whole box is either a control or a gap
 *  with nothing in between. */
function isInteractive(el: HTMLElement): boolean {
  const tag = el.tagName;
  if (tag === 'BUTTON' || tag === 'A' || tag === 'INPUT' || tag === 'SELECT') return true;
  if (el.closest('button, a, input, select, .hamburger-panel, .thread-toggle')) return true;
  return false;
}

/** A click on the thread pane's header. It focuses the pane (the wash) AND puts
 *  the caret in the composer: the header band is the largest neutral surface on
 *  that side, and lighting the wash without moving DOM focus left the user
 *  typing into nothing. Clicks on a control are that control's (isInteractive),
 *  so they keep only the pane focus: Search Everywhere opens an overlay with its
 *  own input, the mark opens the Lucidos menu, and the compose button
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

  // The DOM reads; the attribution itself is pure (headerDblClickRegion), which
  // is also where the drawer segment is ruled out as a surface that answers.
  const styles = getComputedStyle(document.documentElement);
  const drawerWidthPx = parseFloat(styles.getPropertyValue('--content-offset') || '0');
  const drawerDividerPx = threadDrawerOpen.value
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

  // The bar renders the short form and the tooltip carries the full name, so a
  // shorthand never hides what the destination is really called. The content
  // header shrinks with the split, so this is the same narrow surface the phone
  // has, just reached by dragging the divider instead.
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
          <div class="thread-header-elements">
            <ThreadsHeader />
            {/* The drawer toggle, in ONE host for both drawer states. It used to
                exist twice, once here and once inside the brand, with CSS keyed
                on data-thread-drawer-open crossfading the pair. That read as the
                user described it: an icon that had never been on screen sat
                waiting at the header's leading edge and faded UP from nothing
                while its twin slid toward it fading DOWN, so for most of the
                slide the header carried two half-transparent icons that ended up
                on nearly the same x. One element that TRAVELS between the two
                positions (shell.css) is the same animation without either of
                those: it is a part of the header the whole way, at full opacity.

                Focus on click, not pointerdown, so a window drag never shifts
                focus — see ThreadsHeader. */}
            <div class="thread-toggle-slot" onClick={onThreadHeaderClick}>
              <ThreadToggleButton />
            </div>
            {/* Focus on click, not pointerdown, so a window drag never shifts
                focus — see ThreadsHeader. */}
            <span class="pane-header-brand" onClick={onThreadHeaderClick}>
              {/* The Lucidos mark, absolutely centred on the pane: brand,
                  connection light and menu in one control, the same one both
                  mobile headers carry. It replaced a `[Lucidos * workspace]`
                  wordmark; the name survives beside it, as a plain label that
                  hides itself when the pane is too narrow to hold it.
                  The thread chevrons FLANK it, which is the arrangement both
                  mobile headers use: history is about the thing in the middle,
                  so it reads better bracketing the brand than lined up in the
                  leading cluster with the drawer toggle, which is about the
                  pane beside it. */}
              <span class="pane-header-brand-label">
                <ThreadBackButton showTooltip />
                {/* One flex item, so the space-between that pins the chevrons
                    to the span's ends leaves the brand whole in the middle
                    instead of pushing the name off the mark. It is also the box
                    WorkspaceNameLabel measures itself against. */}
                <span class="pane-header-brand-center">
                  <BrandMenuButton placement="brand" />
                  <WorkspaceNameLabel />
                </span>
                <ThreadForwardButton showTooltip />
              </span>
              {/* Right-side actions, and the region's ONLY in-flow child now
                  that the drawer toggle has its own host: the brand pins them to
                  its trailing edge with justify-content, while the label floats
                  over the pane's true middle. They fold into a ⋯ menu as the
                  split narrows, see ThreadHeaderActions. */}
              <ThreadHeaderActions />
            </span>
          </div>
          {/* Focus on click, not pointerdown, so a window drag never shifts
              focus — see ThreadsHeader. */}
          <div class="content-header-elements" onClick={() => focusPane('content')}>
            <HamburgerButton />
            {/* Same arrangement as the thread pane one row over, and as both
                mobile headers: the chevrons bracket the title rather than
                sitting in the leading cluster, so history reads as belonging to
                what is on screen. The title is the group's one shrinking
                member, and the group is a FIXED SPAN, so the chevrons hold
                their places as the user navigates between destinations with
                wildly different title lengths. */}
            <span class="pane-header-content-title">
              <span class="header-title-span">
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
            </span>
            <ContentHeaderActions layout="desktop" />
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
