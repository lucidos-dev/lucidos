import type { ComponentType } from 'preact';
import { useState, useRef, useCallback } from 'preact/hooks';
import { ConnectionStatus } from './ConnectionStatus';
import { createComposeDraft } from '../../store/actions/drafts';
import { composeHandlers } from '../chat/promptFocus';
import { ComposeIcon, GridIcon, SearchIcon, FilterIcon, ThreadsIcon } from '../shared/icons';
import { ThreadNav } from '../shared/ThreadNav';
import { SearchEverywhereButton } from '../shared/SearchEverywhereButton';
import { HamburgerButton, ContentBackButton, ContentForwardButton } from './PanelNav';
import { ContentHeaderActions } from './ContentHeaderActions';
import { ControlPanel, controlPanelOpen, controlPanelBadgeCount, controlPanelBadgeTooltip } from './ControlPanel';
import { ThreadFilterDropdown } from './ThreadFilterDropdown';
import { getContentTitle, getDiffDescription } from './headerHelpers';
import { attentionThreadCount, threadSearchQuery, mobileView, MOBILE_VIEWS, threadChannelFilter, ALL_CHANNELS, unreadCount, changes, focusedThreadId, threadMap, effectiveThreadStatus, type MobileView } from '../../store/store';
import { navigateToPane, toggleThreads } from '../../store/actions/pane';
import { useThreadSearch } from '../../hooks/useThreadSearch';
import { ThreadTitleEditor } from '../chat/ThreadTitleEditor';
import { ThreadStatusIcon, resolveVisualStatus } from '../shared/ThreadStatusIcon';
import { PinButton } from '../shared/PinButton';
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
  content: { Header: MobileContentHeader, Pane: ContentPane },
};

/** Mobile threads header — search and filter for the threads pane */
export function MobileThreadsHeader() {
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
        <span class="icon-btn header-icon" style={{ visibility: 'hidden' }} aria-hidden="true" />
        <span class="icon-btn header-icon" style={{ visibility: 'hidden' }} aria-hidden="true" />
        <span class="pane-header-title mobile-header-title">Threads</span>
        <div class="pane-header-spacer" />
        <button
          class="icon-btn header-icon brand-compose-btn"
          {...composeHandlers(() => { createComposeDraft(); navigateToPane('thread'); })}
          aria-label="New thread"
        >
          <ComposeIcon />
        </button>
        <div style={{ position: 'relative' }}>
          <button
            ref={toggleRef}
            class={`icon-btn header-icon${filterActive ? ' filter-active' : ''}`}
            onClick={() => setFilterOpen(!filterOpen)}
            aria-label="Filter threads"
          >
            <FilterIcon />
          </button>
          {filterOpen && <ThreadFilterDropdown onClose={closeFilter} toggleRef={toggleRef} />}
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

/** Mobile thread header — brand mode with thread grid icon to navigate to threads pane */
export function MobileThreadHeader() {
  const badgeCount = controlPanelBadgeCount();
  const contentBadge = unreadCount.value + changes.value.length;

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
          {...composeHandlers(() => createComposeDraft())}
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
          {contentBadge > 0 && (
            <span class="badge">{contentBadge > 99 ? '99+' : contentBadge}</span>
          )}
        </button>
      </div>
    </div>
  );
}

/** Mobile content header — same as desktop content side */
export function MobileContentHeader() {
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
  const threadTitle = eventThread?.meta.title || '';
  const visualStatus = eventThread
    ? resolveVisualStatus(
        effectiveThreadStatus(eventThread),
        eventThread.meta.activeChildrenCount > 0,
        eventThread.meta.ccHasChanges,
      )
    : undefined;

  if (!threadId) return null;

  return (
    <div class="mobile-thread-title-row">
      <PinButton threadId={threadId} pinned={eventThread?.meta.pinned ?? false} />
      <ThreadTitleEditor threadId={threadId} title={threadTitle} />
      {visualStatus && <ThreadStatusIcon status={visualStatus} />}
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
