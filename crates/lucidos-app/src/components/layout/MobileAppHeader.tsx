import type { ComponentType } from 'preact';
import { ConnectionStatus } from './ConnectionStatus';
import { unfocusThread } from '../../store/actions/threads';
import { composeHandlers } from '../chat/promptFocus';
import { scrolledFromTop } from '../chat/scrollState';
import { ComposeIcon, SearchIcon } from '../shared/icons';
import { PinThreadButton } from '../shared/PinThreadButton';
import { ThreadOverflowMenu } from '../shared/ThreadOverflowMenu';
import { ThreadNav } from '../shared/ThreadNav';
import { ThreadToggleButton } from '../shared/ThreadToggleButton';
import { SearchEverywhereButton } from '../shared/SearchEverywhereButton';
import { HamburgerButton, ContentBackButton, ContentForwardButton } from './PanelNav';
import { ContentHeaderActions } from './ContentHeaderActions';
import { ControlPanel, BrandBadge, toggleControlPanelAtClick } from './ControlPanel';
import { ThreadFilterDropdown, viewIcon } from './ThreadFilterDropdown';
import { getContentTitle, getDiffDescription } from './headerHelpers';
import { threadSearchQuery, mobileView, MOBILE_VIEWS, focusedThreadId, threadMap, drawerView, attentionThreadCount, type MobileView } from '../../store/store';
import { navigateToPane } from '../../store/actions/pane';
import { useThreadsHeaderState } from '../../hooks/useThreadsHeaderState';
import { ThreadTitleEditor } from '../chat/ThreadTitleEditor';
import { ThreadStatusIcon, threadVisualStatus } from '../shared/ThreadStatusIcon';
import { threadDisplayTitle } from '../../utils/threadTitle';
import { MobileThreadsPane } from './MobileThreadsPane';
import { ThreadPane } from './ThreadPane';
import { ContentPane } from './ContentPane';

/** Configuration pairing a header with its corresponding pane component.
 *  Record<MobileView, MobilePaneConfig> makes it a compile error to add a
 *  pane without a header or vice versa. */
export interface MobilePaneConfig {
  Header: ComponentType;
  Pane: ComponentType;
}

/** Single source of truth pairing every MobileView with its Header and Pane.
 *  Defined AFTER the header/pane components below — hoisted by JS. */
export const MOBILE_PANE_CONFIGS: Record<MobileView, MobilePaneConfig> = {
  threads: { Header: MobileThreadsHeader, Pane: MobileThreadsPane },
  thread:  { Header: MobileThreadHeader,  Pane: ThreadPane },
  content: { Header: MobileContentHeader, Pane: () => <ContentPane layout="mobile" /> },
};

/** Mobile threads header — search and filter for the threads pane */
function MobileThreadsHeader() {
  const { filterOpen, setFilterOpen, toggleRef, closeFilter, filterActive,
          searchOpen, searchInputRef, onSearchInput, onSearchKeyDown, closeSearch, openSearchHandlers } = useThreadsHeaderState();

  // The unified Filter control is active when a non-default drawer view is
  // selected OR a channel filter is set. The needs-attention badge rides on the
  // same button (attention-only).
  const filterButtonActive = drawerView.value !== 'all' || filterActive;
  const attentionCount = attentionThreadCount.value;
  // The button glyph reflects the selected view (funnel for `all`).
  const ViewIcon = viewIcon(drawerView.value);

  return (
    <div class={`mobile-threads-header${searchOpen ? ' search-active' : ''}`}>
      <div class="mobile-header-row">
        <div class="mobile-thread-search-bar">
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
          <button class="icon-btn header-icon" onClick={closeSearch} aria-label="Close search">
            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round">
              <path d="M4 4l8 8M12 4l-8 8" />
            </svg>
          </button>
        </div>
        {/* Single unified Filter control — opens the merged View + Show
            dropdown (see ThreadFilterDropdown). Packed left in the same slot the
            channel filter + view selector used to share. */}
        <div class="view-selector-slot">
          <button
            ref={toggleRef}
            class={`icon-btn header-icon${filterButtonActive ? ' view-selector-active' : ''}`}
            onClick={() => setFilterOpen(!filterOpen)}
            aria-label="Filter threads"
            aria-haspopup="menu"
            aria-expanded={filterOpen}
          >
            <ViewIcon />
            {attentionCount > 0 && <span class="badge">{attentionCount}</span>}
          </button>
          {filterOpen && <ThreadFilterDropdown onClose={closeFilter} toggleRef={toggleRef} />}
        </div>
        {/* Title is absolutely centered on the row middle (see
            .mobile-header-title); the spacer pins the trailing icons right. */}
        <span class="pane-header-title mobile-header-title">Threads</span>
        <div class="pane-header-spacer" />
        <button
          class="icon-btn header-icon brand-compose-btn"
          {...composeHandlers(() => unfocusThread())}
          aria-label="New thread"
        >
          <ComposeIcon />
        </button>
        <button
          class="icon-btn header-icon"
          {...openSearchHandlers}
          aria-label="Search threads"
        >
          <SearchIcon />
        </button>
      </div>
    </div>
  );
}

/** Mobile thread header (brand mode). The leading control is the same thread
 *  drawer toggle desktop puts here (ThreadToggleButton: same glyph, and
 *  `toggleThreads` navigates to the threads pane on mobile), so the
 *  needs-attention badge that rides it is visible from the conversation rather
 *  than only from the threads pane's own Filter button. The hamburger keeps its
 *  place on this row but moves to the far TRAILING edge, mirroring the toggle
 *  across the row: both drawers stay one tap from the conversation, and the menu
 *  drawer slides out from under it on the right (drawerSideFor in Drawer.tsx).
 *  Pane navigation is otherwise swipe-only; the dot indicator remains as a
 *  tappable cue. */
function MobileThreadHeader() {
  return (
    <div class="mobile-thread-header">
      <div class="mobile-header-row">
        <ThreadToggleButton />
        <div class="mobile-nav-slot"><ThreadNav /></div>
        {/* Brand is absolutely centered on the row middle (see
            .mobile-header-title); the spacer pins the trailing icons right. */}
        <span class="pane-header-brand mobile-header-title">
          <span
            class="pane-header-brand-label"
            data-role="control-panel-toggle"
            onClick={(e) => {
              if (e.target === e.currentTarget) return;
              toggleControlPanelAtClick(e);
            }}
          >
            <span class="pane-header-title">Lucidos</span>
            <BrandBadge />
            <ConnectionStatus />
          </span>
          <ControlPanel layout="mobile" />
        </span>
        <div class="pane-header-spacer" />
        <button
          class="icon-btn header-icon brand-compose-btn"
          {...composeHandlers(() => unfocusThread())}
          aria-label="New thread"
        >
          <ComposeIcon />
        </button>
        <SearchEverywhereButton />
        <HamburgerButton />
      </div>
    </div>
  );
}

/** Mobile content header — same as desktop content side */
function MobileContentHeader() {
  const title = getContentTitle();
  const diffDesc = getDiffDescription();

  return (
    <div class="mobile-content-header">
      <div class="mobile-header-row">
        <HamburgerButton />
        <div class="mobile-nav-slot">
          <ContentBackButton />
          <ContentForwardButton />
        </div>
        {/* Title is absolutely centered on the row middle (see
            .mobile-header-title); the spacer pins the trailing actions right
            (and keeps them right-aligned when there's no title). */}
        {title && <span class="pane-header-title mobile-header-title mobile-content-title" data-tooltip={diffDesc || title} data-tooltip-tap>{title}</span>}
        <div class="pane-header-spacer" />
        <ContentHeaderActions />
      </div>
    </div>
  );
}

/** Three-dot indicator for mobile view switching. Tappable.
 *  Always visible — hiding them when drawers are open traps the user. */
export function MobileDotIndicator() {
  const view = mobileView.value;

  return (
    <div class="mobile-dot-indicator">
      {MOBILE_VIEWS.map((v) => (
        <button
          key={v}
          class={`mobile-dot${view === v ? ' active' : ''}`}
          onClick={() => navigateToPane(v)}
          aria-label={`${v} view`}
        />
      ))}
    </div>
  );
}

/** Thread title bar — rendered inside the thread pane scroll container so it
 *  swipes with the pane, while using position:sticky + CSS vars to track the
 *  app header's hide/show state. */
export function MobileThreadTitleBar() {
  const threadId = focusedThreadId.value;
  const eventThread = threadId ? threadMap.value.get(threadId) : undefined;
  if (!threadId || !eventThread) return null;

  const threadTitle = threadDisplayTitle(eventThread);
  const visualStatus = threadVisualStatus(eventThread);

  return (
    <div class={`mobile-thread-title-row${scrolledFromTop.value ? ' scrolled' : ''}`}>
      <ThreadStatusIcon status={visualStatus} />
      <ThreadTitleEditor threadId={threadId} title={threadTitle} />
      <span class="thread-view-header-actions">
        {eventThread.meta.state !== 'composing' && (
          <PinThreadButton threadId={threadId} saved={eventThread.meta.saved} />
        )}
        <ThreadOverflowMenu threadId={threadId} title={threadTitle} />
      </span>
    </div>
  );
}

/** Mobile-only header sections, rendered inside the shared <header> element.
 *  Headers render from MOBILE_PANE_CONFIGS — same registry that drives pane
 *  rendering in MobileSwipeContainer. */
export function MobileAppHeader() {
  return (
    <>
      {MOBILE_VIEWS.map((v) => {
        const { Header } = MOBILE_PANE_CONFIGS[v];
        return <Header key={v} />;
      })}
      <MobileDotIndicator />
    </>
  );
}
