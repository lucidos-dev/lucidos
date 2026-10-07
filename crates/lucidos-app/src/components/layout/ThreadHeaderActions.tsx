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

/** The boxes the thread row's collapse is measured against: the region, and the
 *  brand cluster centred on it. The room these actions have is half of what
 *  that cluster leaves, not the region's leftover, so the leading end (the
 *  drawer toggle, and Home) needs no measuring. Stable identity so the collapse
 *  effect's deps do not re-fire every render. */
const COLLAPSE_TARGETS: HeaderCollapseTargets = {
  container: '.pane-header-brand',
  centre: '.pane-header-brand-label',
  // Present only while sessions are resuming, and it never collapses, so the
  // measurement has to budget for it exactly as the content row budgets for the
  // bell. It is a bare glyph rather than an icon button, which is why the
  // anchor is measured at its own width.
  anchor: '.recovery-indicator',
};

/** The thread pane header's actions, as data, so a narrowing pane can fold them
 *  into a ⋯ menu instead of crowding the mark.
 *
 *  Ordered nearest-centre first, which is the end collapse eats from: Setup
 *  interview goes first (a once-or-twice thing), then New thread. Search
 *  everywhere is the last one standing at the row's outer edge, where the
 *  pointer already is. Home is not one of these: it leads the row instead
 *  (`ThreadHeaderHomeButton`).
 *
 *  Exported as a pure function so the set and its order are testable without
 *  standing the header up. */
export function threadHeaderActions(): HeaderActionSpec[] {
  const actions: HeaderActionSpec[] = [];

  // Gated on a configured LLM, like the menu row that mirrors it: the interview
  // is a conversation, and there is nothing to have it with.
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
    key: 'new-thread',
    label: 'New thread',
    tooltip: tooltipWithShortcut('New thread', 'newThread'),
    icon: () => <ComposeIcon />,
    onClick: () => unfocusThread(),
    extraClass: 'brand-compose-btn',
  });

  actions.push({
    key: 'search-everywhere',
    label: 'Search everywhere',
    tooltip: tooltipWithShortcut('Search everywhere', 'searchEverywhere'),
    icon: () => <SearchIcon />,
    onClick: (e) => {
      // The palette's dismiss anchor is the control that opened it, so that
      // re-activating it closes via this handler instead of racing the
      // outside-pointerdown dismiss (.claude/rules/frontend.md). That only holds
      // for the HEADER rendering: a ⋯ menu row unmounts the moment the menu
      // closes, and a detached anchor exempts nothing. `icon-btn` is on the
      // header rendering alone, which is how the two are told apart here.
      const el = e.currentTarget as HTMLElement;
      searchEverywhereAnchor.value = el.classList.contains('icon-btn') ? el : null;
      focusSearchInput();
      searchEverywhereOpen.value = !searchEverywhereOpen.value;
    },
    extraClass: 'search-everywhere-btn',
  });

  return actions;
}

/** The Home action, which leads the desktop thread pane's header rather than
 *  joining the trailing cluster. Home has no row in the thread list, so this is
 *  the way to it. Present only while the experimental switch is on and the
 *  thread exists. */
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

/** The leading control of the DESKTOP thread pane's header. It never folds:
 *  the brand cluster's reserve and the pane's floor both count its box. */
export function ThreadHeaderHomeButton() {
  const home = threadHeaderHomeAction();
  return home ? renderHeaderAction(home) : null;
}

/**
 * The trailing cluster of the DESKTOP thread pane's header.
 *
 * Desktop only: `.desktop-header` is `display: none` under the mobile
 * breakpoint, and neither mobile header carries these actions (the phone reaches
 * them from the Lucidos menu instead, which is why the menu drops them here).
 *
 * It collapses progressively, the same way the content pane's does and for the
 * same reason: the thread pane is one side of a draggable split, so its header
 * runs out of room without the window changing size at all. Dragging the divider
 * narrow used to crowd the icons against the centred mark; now they fold into ⋯
 * one step at a time and the brand keeps its place.
 */
export function ThreadHeaderActions() {
  const hostRef = useRef<HTMLSpanElement>(null);
  const actions = threadHeaderActions();
  const collapsed = useHeaderActionCollapse(hostRef, actions.length, 'desktop', COLLAPSE_TARGETS);
  const recovery = recoveryProgress.value;

  return (
    <span class="pane-header-brand-actions" ref={hostRef}>
      <CollapsingActions actions={actions} collapsed={collapsed} moreClass="thread-header-more">
        {/* Not an action: a transient status glyph, present only while sessions
            are resuming, and nothing to fold into a menu of things to do. */}
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
