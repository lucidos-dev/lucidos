import type { ComponentType } from 'preact';
import { ConnectionStatus } from './ConnectionStatus';
import { unfocusThread } from '../../store/actions/threads';
import { composeHandlers } from '../chat/promptFocus';
import { scrolledFromTop } from '../chat/scrollState';
import { ComposeIcon, SearchIcon, FilterIcon } from '../shared/icons';
import { AltViewToggles } from '../shared/AltViewToggles';
import { CopyThreadRefButton } from '../shared/CopyThreadRefButton';
import { ExportThreadButton } from '../shared/ExportThreadButton';
import { ThreadNav } from '../shared/ThreadNav';
import { SearchEverywhereButton } from '../shared/SearchEverywhereButton';
import { HamburgerButton, ContentBackButton, ContentForwardButton } from './PanelNav';
import { ContentHeaderActions } from './ContentHeaderActions';
import { ControlPanel, controlPanelOpen, controlPanelAnchor, controlPanelBadgeCount, controlPanelBadgeTooltip } from './ControlPanel';
import { ThreadFilterDropdown } from './ThreadFilterDropdown';
import { getContentTitle, getDiffDescription } from './headerHelpers';
import { threadSearchQuery, mobileView, MOBILE_VIEWS, focusedThreadId, threadMap, draftsViewActive, attentionViewActive, type MobileView } from '../../store/store';
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

  // The channel/trigger/repo filter is unavailable while an alternate filter
  // view (drafts or attention) is running — both deliberately bypass it.
  const altViewActive = draftsViewActive.value || attentionViewActive.value;

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
        <div style={{ position: 'relative' }}>
          <button
            ref={toggleRef}
            class={`icon-btn header-icon${filterActive ? ' filter-active' : ''}`}
            onClick={() => setFilterOpen(!filterOpen)}
            disabled={altViewActive}
            aria-label="Filter threads"
            data-tooltip={altViewActive ? 'Filter unavailable in this view' : 'Filter threads'}
            style={altViewActive ? 'pointer-events: auto;' : undefined}
          >
            <FilterIcon />
          </button>
          {filterOpen && !altViewActive && <ThreadFilterDropdown onClose={closeFilter} toggleRef={toggleRef} />}
        </div>
        {/* Needs-attention + drafts toggles, in the slot the prev/next thread
            arrows used to occupy. Shared scheme with desktop (see
            AltViewToggles): shown only when non-empty, packed left, attention
            first. */}
        <AltViewToggles />
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

/** Mobile thread header — brand mode. Pane navigation is swipe-only (no
 *  threads/content toggle icons); the dot indicator remains as a tappable cue. */
function MobileThreadHeader() {
  const badgeCount = controlPanelBadgeCount();

  return (
    <div class="mobile-thread-header">
      <div class="mobile-header-row">
        <div class="mobile-nav-slot"><ThreadNav /></div>
        <span class="pane-header-brand mobile-header-title">
          <span
            class="pane-header-brand-label"
            data-role="control-panel-toggle"
            onClick={(e) => {
              if (e.target === e.currentTarget) return;
              controlPanelAnchor.value = e.currentTarget as HTMLElement;
              controlPanelOpen.value = !controlPanelOpen.value;
            }}
          >
            <span class="pane-header-title">Lucidos</span>
            {badgeCount > 0 && <span class="badge brand-badge" data-tooltip={controlPanelBadgeTooltip()}>{badgeCount}</span>}
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
  const visualStatus = threadVisualStatus(eventThread);

  return (
    <div class={`mobile-thread-title-row${scrolledFromTop.value ? ' scrolled' : ''}`}>
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
