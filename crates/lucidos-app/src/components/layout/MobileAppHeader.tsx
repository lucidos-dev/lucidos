import type { ComponentType } from 'preact';
import { scrolledFromTop } from '../chat/scrollState';
import { SearchIcon } from '../shared/icons';
import { PinThreadButton } from '../shared/PinThreadButton';
import { ThreadOverflowMenu } from '../shared/ThreadOverflowMenu';
import { ThreadBackButton, ThreadForwardButton } from '../shared/ThreadNav';
import { ThreadToggleButton } from '../shared/ThreadToggleButton';
import { HamburgerButton, ContentBackButton, ContentForwardButton } from './ContentNav';
import { ContentHeaderActions } from './ContentHeaderActions';
import { BrandMenuButton } from './HeaderMark';
import { getContentTitle, getContentTitleShort, getDiffDescription } from './headerHelpers';
import { threadSearchQuery, mobileView, MOBILE_VIEWS, focusedThreadId, threadMap, type MobileView } from '../../store/store';
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
 *  It sits ABOVE the header components it names; a function declaration
 *  hoists, so the references resolve. */
export const MOBILE_PANE_CONFIGS: Record<MobileView, MobilePaneConfig> = {
  threads: { Header: MobileThreadsHeader, Pane: MobileThreadsPane },
  thread:  { Header: MobileThreadHeader,  Pane: ThreadPane },
  content: { Header: MobileContentHeader, Pane: () => <ContentPane layout="mobile" /> },
};

/** Mobile threads header — search and filter for the threads pane */
function MobileThreadsHeader() {
  // Glyph, active highlight and the attention-only badge all come from the
  // shared hook, so this row and the desktop one cannot drift.
  const { filterOpen, toggleFilter, filterButtonActive, FilterButtonIcon, filterButtonBadge,
          searchOpen, searchInputRef, onSearchInput, onSearchKeyDown, closeSearch, openSearchHandlers } = useThreadsHeaderState();

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
        {/* Single unified Filter control: it toggles the merged Status +
            Thread type panel, which renders down in the threads pane itself
            (see ThreadFilterPanel / ThreadDrawer). Packed left in the same slot
            the channel filter + view selector used to share. It is also the
            panel's only way out, which is what the X glyph says while the panel
            is up (see useThreadsHeaderState). */}
        <div class="view-selector-slot">
          <button
            class={`icon-btn header-icon${filterButtonActive ? ' view-selector-active' : ''}`}
            onClick={toggleFilter}
            aria-label="Filter threads"
            aria-expanded={filterOpen}
          >
            <FilterButtonIcon />
            {filterButtonBadge > 0 && <span class="badge">{filterButtonBadge}</span>}
          </button>
        </div>
        {/* Title is absolutely centered on the row middle (see
            .mobile-header-title); the spacer pins the trailing icons right. It
            says what the pane is showing: the list, or the filter panel that has
            taken it over (ThreadFilterPanel carries no title row of its own).
            Just "Filters", matching the desktop row: the pane is already the
            Threads pane. */}
        <span class="pane-header-title mobile-header-title">
          {filterOpen ? 'Filters' : 'Threads'}
        </span>
        <div class="pane-header-spacer" />
        {/* No SetupInterviewButton on either mobile header, deliberately: the
            setup interview is a once-or-twice thing, and a permanent icon for it
            costs a phone's scarcest row more than it is worth. Mobile reaches it
            from the welcome CTA (SetupInterviewWelcome) or by asking in the
            chat, which is what the welcome's hint says on this viewport. */}
        {/* The same menu as the thread pane, so it is reachable from here too,
            but dressed as a member of an icon run rather than as the thread
            pane's centred mark: `placement="row"` puts it on
            `.icon-btn.header-icon`, the class Search beside it uses, which is
            what keeps the two on one rhythm.

            It is POSITIONED by the same fixed-width centred cluster the other
            two rows hang their chevrons off, pinned to that cluster's trailing
            edge, so it lands on the forward chevron's column rather than
            wherever the trailing run's width happened to leave it. The mark is
            the one control on all three mobile rows, and it was the one moving
            as the user swiped between them. Search keeps the trailing edge. */}
        <div class="header-nav-cluster header-mark-end-cluster">
          <BrandMenuButton placement="row" />
        </div>
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

/** Mobile thread header. The row is built around a CENTRED cluster of back
 *  chevron, Lucidos mark, forward chevron, with one drawer affordance at each
 *  edge.
 *
 *  The mark is three controls in one: the brand, the connection light, and the
 *  menu carrying New thread, Search everywhere and Workspaces. Those three used
 *  to be a compose button, a search button and the brand label, which is what
 *  makes room for the nav chevrons to move off the leading edge and flank the
 *  mark where a thumb reaches them.
 *
 *  Both edges keep their drawer: the thread drawer toggle leads (so the
 *  needs-attention badge riding it stays visible from the conversation, per
 *  `threadToggleBadgeCount`) and the hamburger trails, mirroring it across the
 *  row so the menu drawer slides out from under it on the right (drawerSideFor
 *  in Drawer.tsx). Pane navigation is otherwise swipe-only; the dot indicator
 *  remains as a tappable cue. */
function MobileThreadHeader() {
  return (
    <div class="mobile-thread-header">
      <div class="mobile-header-row">
        <ThreadToggleButton />
        <div class="pane-header-spacer" />
        {/* Absolutely centred on the row middle rather than between the two
            edge clusters, so the mark sits on the viewport axis (the same rule
            the title followed, see .mobile-header-title in mobile.css). Unlike
            a title this cluster cannot shrink, so its clearance from both edges
            is a fixed-width guarantee, pinned by
            e2e/mobile-threads-title-alignment.spec.ts. */}
        <div class="header-nav-cluster">
          <ThreadBackButton />
          <BrandMenuButton />
          <ThreadForwardButton />
        </div>
        <HamburgerButton />
      </div>
    </div>
  );
}

/** Mobile content header. Repeats the thread row's shape one pane over, and
 *  literally so: the cluster is the same fixed-width centred box, so the two
 *  chevrons land on the same two points of the screen as the thread pane's and
 *  navigation does not move under the thumb when the user swipes between panes.
 *
 *  The title is the cluster's one shrinking member, so a long one ellipsises
 *  between the chevrons rather than pushing either into an edge control. With
 *  no title the cluster is just the two chevrons, in the same places. What
 *  makes the fixed span possible is `ContentHeaderActions` collapsing to a
 *  single control plus the bell: a trailing cluster bounded at two icon boxes,
 *  whether that control is the ⋯ trigger or a view's one context action.
 *
 *  That span is around a dozen characters, so a destination WE name renders its
 *  authored short form (`SettingsNavItem.short`) rather than ellipsising a name
 *  we could have written shorter. The ellipsis stays underneath for the names
 *  we do not author: files, apps, web pages, threads. Either way the tap
 *  tooltip carries the full title. */
function MobileContentHeader() {
  const title = getContentTitleShort();
  const titleFull = getContentTitle();
  const diffDesc = getDiffDescription();

  return (
    <div class="mobile-content-header">
      <div class="mobile-header-row">
        <HamburgerButton />
        <div class="pane-header-spacer" />
        <div class="header-nav-cluster header-title-cluster">
          <ContentBackButton />
          {title && (
            <span
              class="pane-header-title mobile-content-title"
              data-tooltip={diffDesc || titleFull}
              data-tooltip-tap
            >
              {title}
            </span>
          )}
          <ContentForwardButton />
        </div>
        <ContentHeaderActions layout="mobile" />
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
    // data-scroller-pinned: the transcript's iOS repaint nudge compensates its
    // own 1px scroll with a transform on the scroll container, which is exact
    // for content the scroll moved. Sticky means the scroll moved this row not
    // at all, so it undoes that compensation itself (utils/webkitRepaint.ts).
    <div
      class={`mobile-thread-title-row${scrolledFromTop.value ? ' scrolled' : ''}`}
      data-scroller-pinned
    >
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
