import type { ComponentChildren, ComponentType } from 'preact';
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
import { lazyComponent } from '../../utils/lazyComponent';

const ThreadUsersButton = lazyComponent(() => import('../chat/ThreadUsers').then((m) => m.ThreadUsersButton));
import { threadVisualStatus } from '../shared/threadVisualStatus';
import { threadDisplayTitle } from '../../utils/threadTitle';
import { MobileThreadsPane } from './MobileThreadsPane';
import { ThreadPane } from './ThreadPane';
import { DashboardPanelToggle } from '../dashboard/DashboardPanel';
import { ContentPane } from './ContentPane';

export interface MobilePaneConfig {
  Header: ComponentType;
  Pane: ComponentType;
}

export const MOBILE_PANE_CONFIGS: Record<MobileView, MobilePaneConfig> = {
  threads: { Header: MobileThreadsHeader, Pane: MobileThreadsPane },
  thread:  { Header: MobileThreadHeader,  Pane: ThreadPane },
  content: { Header: MobileContentHeader, Pane: () => <ContentPane layout="mobile" /> },
};

function MobileThreadsHeader() {
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
        <ThreadGroupingButton />
        <ThreadFilterButton />
        <ThreadsPaneTitle class="pane-header-title mobile-header-title" />
        <div class="pane-header-spacer" />
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

function MobileThreadHeader() {
  return (
    <div class="mobile-thread-header">
      <div class="mobile-header-row">
        <ThreadToggleButton />
        <div class="pane-header-spacer" />
        <div class="header-nav-cluster">
          <ThreadBackButton />
          <span class="header-mark-group">
            <ThreadHeaderHomeButton />
            <BrandMenuButton />
            <DashboardPanelToggle />
          </span>
          <ThreadForwardButton />
        </div>
        <HamburgerButton />
      </div>
    </div>
  );
}

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

export function MobileThreadTitleBar({ usersPanel }: { usersPanel?: ComponentChildren } = {}) {
  const threadId = focusedThreadId.value;
  const eventThread = threadId ? threadMap.value.get(threadId) : undefined;
  if (!threadId || !eventThread) return null;

  const threadTitle = threadDisplayTitle(eventThread);

  return (
    <div
      class={`mobile-thread-title-row${scrolledFromTop.value ? ' scrolled' : ''}`}
      data-scroller-pinned
    >
      <ThreadTitleMenu thread={eventThread} title={threadTitle} status={threadVisualStatus(eventThread)} />
      <ThreadUsersButton threadId={threadId} />
      {usersPanel && <div class="mobile-thread-users-panel">{usersPanel}</div>}
    </div>
  );
}

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
