import type { ComponentType } from 'preact';
import { useState, useRef, useCallback } from 'preact/hooks';
import { ConnectionStatus } from './ConnectionStatus';
import { unfocusThread } from '../../store/actions/threads';
import { composeHandlers } from '../chat/promptFocus';
import { ComposeIcon, GridIcon, SearchIcon, FilterIcon, ThreadsIcon, DraftsIcon } from '../shared/icons';
import { CopyThreadRefButton } from '../shared/CopyThreadRefButton';
import { ExportThreadButton } from '../shared/ExportThreadButton';
import { ThreadNav } from '../shared/ThreadNav';
import { SearchEverywhereButton } from '../shared/SearchEverywhereButton';
import { HamburgerButton, ContentBackButton, ContentForwardButton } from './PanelNav';
import { ContentHeaderActions } from './ContentHeaderActions';
import { ControlPanel, controlPanelOpen, controlPanelBadgeCount, controlPanelBadgeTooltip } from './ControlPanel';
import { ThreadFilterDropdown } from './ThreadFilterDropdown';
import { getContentTitle, getDiffDescription } from './headerHelpers';
import { attentionThreadCount, threadSearchQuery, mobileView, MOBILE_VIEWS, threadChannelFilter, ALL_CHANNELS, unreadCount, focusedThreadId, threadMap, effectiveThreadStatus, draftsViewActive, type MobileView } from '../../store/store';
import { navigateToPane, toggleThreads } from '../../store/actions/pane';
import { useThreadSearch } from '../../hooks/useThreadSearch';
import { ThreadTitleEditor } from '../chat/ThreadTitleEditor';
import { ThreadStatusIcon, resolveVisualStatus } from '../shared/ThreadStatusIcon';
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
  const [filterOpen, setFilterOpen] = useState(false);
  const { searchOpen, searchInputRef, onSearchInput, onSearchKeyDown, closeSearch, openSearchHandlers } = useThreadSearch();
  const toggleRef = useRef<HTMLButtonElement>(null);
  const closeFilter = useCallback(() => setFilterOpen(false), []);
  const filterActive = threadChannelFilter.value.size < ALL_CHANNELS.length;

  return (
    <div class={`mobile-threads-header${searchOpen ? ' search-active' : ''}`}>
      <div class="mobile-header-row">
        <div class="mobile-thread-search-bar">
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
          <button class="icon-btn header-icon" onClick={closeSearch} aria-label="Close search">
            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round">
              <path d="M4 4l8 8M12 4l-8 8" />
            </svg>
          </button>
        </div>
        <div style={{ position: 'relative' }}>
          <button
            ref={toggleRef}
            class={`icon-btn header-icon${filterActive ? ' filter-active' : ''}`}
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
        <div class="mobile-nav-slot"><ThreadNav keepCurrentPane /></div>
        <span class="pane-header-title mobile-header-title">Threads</span>
        <div class="pane-header-spacer" />
        <button
          class="icon-btn header-icon brand-compose-btn"
          {...composeHandlers(() => { unfocusThread(); navigateToPane('thread'); })}
          aria-label="New thread"
        >
          <ComposeIcon />
        </button>
        <button
          class={`icon-btn header-icon${draftsViewActive.value ? ' drafts-active' : ''}`}
          onClick={() => { draftsViewActive.value = !draftsViewActive.value; }}
          aria-label="Toggle drafts view"
        >
          <DraftsIcon />
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

/** Mobile thread header — brand mode with thread grid icon to navigate to threads pane */
function MobileThreadHeader() {
  const badgeCount = controlPanelBadgeCount();

  return (
    <div class="mobile-thread-header">
      <div class="mobile-header-row">
        <button
          class="icon-btn header-icon thread-toggle"
          onClick={() => toggleThreads()}
          aria-label="Show threads"
        >
          <ThreadsIcon />
          {attentionThreadCount.value > 0 && (
            <span class="badge">{attentionThreadCount.value}</span>
          )}
        </button>
        <div class="mobile-nav-slot"><ThreadNav /></div>
        <span class="pane-header-brand mobile-header-title">
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
        <button
          class="icon-btn header-icon"
          onClick={() => navigateToPane('content')}
          aria-label="Show content"
        >
          <GridIcon />
          {unreadCount.value > 0 && (
            <span class="badge">{unreadCount.value > 99 ? '99+' : unreadCount.value}</span>
          )}
        </button>
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
  const visualStatus = resolveVisualStatus(
    effectiveThreadStatus(eventThread),
    eventThread.meta.activeChildrenCount > 0,
    eventThread.meta.ccHasChanges,
  );

  return (
    <div class="mobile-thread-title-row">
      <ThreadStatusIcon status={visualStatus} />
      <ThreadTitleEditor threadId={threadId} title={threadTitle} />
      <span class="thread-view-header-actions">
        <CopyThreadRefButton threadId={threadId} title={threadTitle} />
        <ExportThreadButton threadId={threadId} title={threadTitle} />
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
