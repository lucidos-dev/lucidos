import { useRef } from 'preact/hooks';
import { llmConfigured, recoveryProgress, searchEverywhereAnchor, searchEverywhereOpen } from '../../store/store';
import { unfocusThread } from '../../store/actions/threads';
import { homeThreadId, openHomeThread } from '../../store/actions/homeThread';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { confirmAndStartSetupInterview } from '../shared/setupInterview';
import { focusSearchInput } from '../search/searchEverywhereActions';
import { ComposeIcon, HelpIcon, HomeIcon, SearchIcon } from '../shared/icons';
import { CollapsingActions, renderHeaderAction, type HeaderActionSpec } from './headerActions';
import { useHeaderActionCollapse, type HeaderCollapseTargets } from '../../hooks/useHeaderActionCollapse';

const COLLAPSE_TARGETS: HeaderCollapseTargets = {
  container: '.pane-header-brand',
  centre: '.pane-header-brand-label',
  anchor: '.recovery-indicator',
};

export function threadHeaderActions(): HeaderActionSpec[] {
  const actions: HeaderActionSpec[] = [];

  if (llmConfigured.value) {
    actions.push({
      key: 'setup-interview',
      label: 'Get the most out of Lucidos',
      tooltip: 'Get the most out of Lucidos: a few questions, then we build what fits',
      icon: () => <HelpIcon />,
      onClick: () => { void confirmAndStartSetupInterview(); },
      extraClass: 'setup-interview-btn',
    });
  }

  actions.push({
    key: 'search-everywhere',
    label: 'Search everywhere',
    tooltip: tooltipWithShortcut('Search everywhere', 'searchEverywhere'),
    icon: () => <SearchIcon />,
    onClick: (e) => {
      const el = e.currentTarget as HTMLElement;
      searchEverywhereAnchor.value = el.classList.contains('icon-btn') ? el : null;
      focusSearchInput();
      searchEverywhereOpen.value = !searchEverywhereOpen.value;
    },
    extraClass: 'search-everywhere-btn',
  });

  return actions;
}

export function threadHeaderHomeAction(): HeaderActionSpec | null {
  if (!homeThreadId.value) return null;
  return {
    key: 'home',
    label: 'Home',
    icon: () => <HomeIcon />,
    onClick: openHomeThread,
    extraClass: 'home-thread-btn',
  };
}

export function ThreadHeaderNewThreadButton() {
  return renderHeaderAction({
    key: 'new-thread',
    label: 'New thread',
    tooltip: tooltipWithShortcut('New thread', 'newThread'),
    icon: () => <ComposeIcon />,
    onClick: () => unfocusThread(),
    extraClass: 'brand-compose-btn',
  });
}

export function ThreadHeaderHomeButton() {
  const home = threadHeaderHomeAction();
  return home ? renderHeaderAction(home) : null;
}

export function ThreadHeaderActions() {
  const hostRef = useRef<HTMLSpanElement>(null);
  const actions = threadHeaderActions();
  const collapsed = useHeaderActionCollapse(hostRef, actions.length, 'desktop', COLLAPSE_TARGETS);
  const recovery = recoveryProgress.value;

  return (
    <span class="pane-header-brand-actions" ref={hostRef}>
      <CollapsingActions actions={actions} collapsed={collapsed} moreClass="thread-header-more">
        {recovery && (
          <span
            class="recovery-indicator"
            data-tooltip={`Resuming sessions: ${recovery.completed}/${recovery.total}`}
          >
            <svg class="recovery-spinner" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M8 2a6 6 0 1 1-4.24 1.76" stroke-linecap="round" />
            </svg>
          </span>
        )}
      </CollapsingActions>
    </span>
  );
}
