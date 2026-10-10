import type { ComponentType } from 'preact';
import { WidgetShelf } from '../widgets/WidgetShelf';
import { scrolledFromTop } from '../chat/scrollState';
import { SearchIcon } from '../shared/icons';
import { SearchField } from '../shared/SearchField';
import { MobileRefreshIndicator } from './RefreshIndicator';
import { ThreadBackButton, ThreadForwardButton } from '../shared/ThreadNav';
import { ThreadToggleButton } from '../shared/ThreadToggleButton';
import { HamburgerButton, ContentBackButton, ContentForwardButton } from './ContentNav';
import { ContentHeaderActions } from './ContentHeaderActions';
import { BrandMenuButton } from './HeaderMark';
import { ThreadHeaderHomeButton } from './ThreadHeaderActions';
import { getContentTitle, getContentTitleShort, getDiffDescription } from './headerHelpers';
import { threadSearchQuery, mobileView, MOBILE_VIEWS, focusedThreadId, threadMap, type MobileView } from '../../store/store';
import { navigateToPane } from '../../store/actions/pane';
import { useThreadSearch } from '../../hooks/useThreadSearch';
import { ThreadFilterButton, ThreadGroupingButton, ThreadsPaneTitle } from './ThreadsHeaderControls';
import { ThreadTitleMenu } from '../chat/ThreadTitle';
import { threadVisualStatus } from '../shared/threadVisualStatus';
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

/** Mobile threads header: filter, grouping and search for the threads pane. */
function MobileThreadsHeader() {
  // The Filter and Grouping buttons and the title are the desktop row's own
  // components, so the two rows cannot drift.
  const { searchOpen, searchInputRef, onSearchInput, onSearchKeyDown, closeSearch, openSearchHandlers } = useThreadSearch();

  return (
    <div class={`mobile-threads-header${searchOpen ? ' search-active' : ''}`}>
      <div class="mobile-header-row">
        <div class="mobile-thread-search-bar">
          <SearchField
            inputRef={searchInputRef}
            inputClass="thread-search-input"
            placeholder="Search threads…"
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
        {/* Grouping leads the row, so the one control shown in both groupings
            holds the corner. The Filter control after it toggles the thread
            filter panel, which renders down in the threads pane itself (see
            ThreadFilterPanel / ThreadDrawer). It is also the panel's visible
            way out, which is why it reads as held down while the panel is up
            (see filterButtonState). It shows under Folders only, fading in a
            box it keeps, so the title beside it never moves. */}
        <ThreadGroupingButton />
        <ThreadFilterButton />
        {/* Title is absolutely centered on the row middle (see
            .mobile-header-title); the spacer pins the trailing icons right. It
            names the grouping, or the filter panel that has taken the list over
            (ThreadFilterPanel carries no title row of its own). */}
        <ThreadsPaneTitle class="pane-header-title mobile-header-title" />
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
 *  chevron, Home and the Lucidos mark, forward chevron, with one drawer
 *  affordance at each edge.
 *
 *  The mark is three controls in one: the brand, the connection light, and the
 *  menu carrying New thread, Search everywhere and Workspaces. Those three used
 *  to be a compose button, a search button and the brand label, which is what
 *  makes room for the nav chevrons to move off the leading edge and flank the
 *  mark where a thumb reaches them.
 *
 *  Both edges keep their drawer: the thread drawer toggle leads (so the
 *  Blocked badge riding it stays visible from the conversation, per
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
          {/* Home and the mark centre as one pair between the chevrons. So
              this mark's menu drops its Home row, which the threads row's
              mark keeps. */}
          <span class="header-mark-pair">
            <ThreadHeaderHomeButton />
            <BrandMenuButton />
          </span>
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
  return (
    <div class="mobile-content-header">
      <div class="mobile-header-row">
        <HamburgerButton />
        <MobileRefreshIndicator />
        <div class="pane-header-spacer" />
        <div class="header-nav-cluster header-title-cluster">
          <ContentBackButton />
          <MobileContentTitle />
          <ContentForwardButton />
        </div>
        <ContentHeaderActions layout="mobile" />
      </div>
    </div>
  );
}

/** The content title between the chevrons. */
function MobileContentTitle() {
  const title = getContentTitleShort();
  if (!title) return null;
  return (
    <span
      class="pane-header-title mobile-content-title"
      data-tooltip={getDiffDescription() || getContentTitle()}
      data-tooltip-tap
    >
      {title}
    </span>
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

  return (
    // data-scroller-pinned: the transcript's iOS repaint nudge compensates its
    // own 1px scroll with a transform on the scroll container, which is exact
    // for content the scroll moved. Sticky means the scroll moved this row not
    // at all, so it undoes that compensation itself (utils/webkitRepaint.ts).
    <div
      class={`mobile-thread-title-row${scrolledFromTop.value ? ' scrolled' : ''}`}
      data-scroller-pinned
    >
      <ThreadTitleMenu thread={eventThread} title={threadTitle} status={threadVisualStatus(eventThread)} />
      <WidgetShelf threadId={threadId} />
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
