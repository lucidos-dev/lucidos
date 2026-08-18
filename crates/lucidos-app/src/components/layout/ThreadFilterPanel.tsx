import { Fragment } from 'preact';
import type { ComponentType, VNode } from 'preact';
import { useRef, useEffect } from 'preact/hooks';
import {
  threadChannelFilter, selectedTriggerIds, selectedRepoIds, selectedAppIds, CODING_AGENT_CHANNEL,
  drawerView, setDrawerView, attentionThreadCount, reviewThreadCount, runningThreadCount,
  type DrawerView, type ThreadChannel,
  includeDeletedFilterOptions, setIncludeDeletedFilterOptions,
} from '../../store/store';
import { threadFilterActive, deletedOptionsHidden } from '../../store/threadFilterActive';
import { draftThreadCount } from '../drawer/family-graph';
import { DraftsIcon, AttentionIcon, ReviewIcon, RunningIcon, FilterIcon, CheckIcon, CloseIcon, CodeIcon } from '../shared/icons';
import { LucidosMark } from '../shared/LucidosMark';
import { Explainer } from '../shared/Explainer';
import { CategoryIcon } from '../shared/CategoryIcon';
import { CHANNEL_OPTIONS } from './headerHelpers';
import { toggleChannel, triggerFilterOptions, toggleTriggerId, toggleTriggerChannel, type TriggerFilterOption } from '../../store/triggerFilters';
import { repoFilterOptions, toggleRepoId, toggleCodingAgentChannel, type RepoFilterOption } from '../../store/repoFilters';
import { appFilterOptions, toggleAppId, type AppFilterOption } from '../../store/appFilters';
import { formatShortDateWithYear } from '../../utils/formatTime';

/** Children rendered under an expanded parent share the same shape — all
 *  child options carry id/label/deleted/lastActivity. */
type ChildOption = TriggerFilterOption | RepoFilterOption | AppFilterOption;

type ChildGroup = {
  label: string;
  items: ChildOption[];
  selected: Set<string>;
  onToggleChild: (id: string) => void;
};

type ViewMeta = { view: DrawerView; label: string; Icon: ComponentType<{ size?: string }> };

/** `all` is the default sectioned list, and the odd one out: the other four are
 *  real statuses that narrow the list, while this one is the absence of a status
 *  and the only view the thread-type filter applies in. So it is NOT in the
 *  Status list at all. It lives BELOW the "or" rule, over the channel section
 *  that narrows it (see `ThreadFilterPanel`), which is why the four below are
 *  the whole of `VIEW_META`.
 *
 *  Named separately because it is also what the closed Filter button falls back
 *  to for its default glyph, by name rather than by index. */
const ALL_STATUSES_META: ViewMeta = { view: 'all', label: 'All statuses', Icon: FilterIcon };

/** The four real statuses, in menu order, single-select. Counts come from the
 *  store / family-graph (see `DrawerView` in store.ts). */
const VIEW_META: readonly ViewMeta[] = [
  { view: 'attention', label: 'Needs attention', Icon: AttentionIcon },
  { view: 'review', label: 'Review', Icon: ReviewIcon },
  { view: 'running', label: 'Running', Icon: RunningIcon },
  { view: 'drafts', label: 'Drafts', Icon: DraftsIcon },
];

/** Everything the threads-header Filter button looks like, on both layouts: its
 *  glyph, whether it wears the active-control highlight, and the count on its
 *  needs-attention badge.
 *
 *  One function because the three answers are not independent. CLOSED, the
 *  button REPORTS: it wears the selected view's own icon (the funnel
 *  `FilterIcon` for the default `all`, each view's glyph otherwise), the
 *  highlight when a filter is on, and the badge, so the state of the list is
 *  readable without opening anything. OPEN, it OFFERS THE WAY OUT and nothing
 *  else: an X, no highlight, no badge. The panel it opened is right underneath
 *  saying what the filter is, in full and in words, so repeating "a filter is
 *  on" over the exit glyph describes something the user is already looking at
 *  while crowding the one thing the button now does.
 *
 *  Called by `useThreadsHeaderState`, which feeds it the signals; kept pure here
 *  so it is testable and so VIEW_META stays the one source of the glyphs. */
export function filterButtonState(opts: {
  view: DrawerView;
  panelOpen: boolean;
  /** A channel / trigger / repo / app selection is set (`threadFilterActive`). */
  channelFilterActive: boolean;
  /** Threads stuck waiting on the user (`attentionThreadCount`). */
  attentionCount: number;
}): { Icon: ComponentType<{ size?: string }>; active: boolean; badge: number } {
  if (opts.panelOpen) return { Icon: CloseIcon, active: false, badge: 0 };
  return {
    // `all` is not in VIEW_META (it is not a status), so it resolves through the
    // same fallback as an unrecognized view, and both land on the funnel.
    Icon: (VIEW_META.find(m => m.view === opts.view) ?? ALL_STATUSES_META).Icon,
    active: opts.view !== 'all' || opts.channelFilterActive,
    badge: opts.attentionCount,
  };
}

/** The leading glyph for a Thread type row (Lucidos / Coding Agent /
 *  Triggers). Mirrors the per-thread `ThreadTypeIcon` mapping so a channel wears
 *  the same mark in the filter as the threads it gathers — except the Coding
 *  Agent group is backend-agnostic, so it uses the generic code glyph rather
 *  than a specific Claude/Codex mark. The Lucidos mark renders monochrome here
 *  (`background={false}` drops the gradient tile; CSS tints the squares + spark
 *  with the row color) so it reads as one of the line glyphs, not a brand badge. */
function channelIcon(value: ThreadChannel): VNode {
  if (value === 'trigger') return <CategoryIcon category="triggers" />;
  if (value === CODING_AGENT_CHANNEL) return <CodeIcon />;
  return <LucidosMark size="1rem" background={false} />;
}

/** The unified thread filter: one panel, one single-select set of five, split by
 *  an "or" rule. Above it the four real **statuses** (Needs attention / Review /
 *  Running / Drafts); below it **All statuses**, then the multi-select channel
 *  rows under a **By thread types** heading.
 *
 *  Those channel rows are NOT a sixth option. They narrow "All statuses" rather
 *  than competing with it, so the single-select set's checkmark stays on that
 *  row whatever is ticked. The heading over them takes its OWN accent and its
 *  own checkmark (a different element) while a type is ticked. The row itself
 *  grows a **filtered** note plus an explainer whenever what is being shown
 *  differs from all of it, whichever setting is doing that: the ticked thread
 *  types, or "Include deleted" holding back a deleted trigger / repo / app that
 *  exists (never the switch merely being off, with nothing deleted to include).
 *
 *  The halves are alternatives rather than a stack, which is what the rule says
 *  out loud: the channel rows DIM whenever a non-`all` status is active, since
 *  those views bypass the channel filter. Dim only, never disabled: the picks
 *  are kept and apply the moment the user takes "All statuses".
 *
 *  It renders as a VIEW INSIDE THE THREAD DRAWER PANE (see `ThreadDrawer`),
 *  covering the pane's list area while it is up, rather than as the anchored
 *  dropdown it used to be. That is why it is NOT an `<Overlay>`: nothing floats
 *  over the thread or content panes, so a click over there is the user's own
 *  click and must not be dismissed-and-swallowed, and nothing behind goes inert.
 *  The one overlay behavior it keeps is Escape, registered on the central
 *  `overlayStack` alongside the open state itself (store/threadFilterPanel.ts),
 *  restores from localStorage included.
 *
 *  Being up IS a state of the drawer, so it survives a reload the way the
 *  drawer's selected view and channel selection do: a reload lands the user back
 *  on the filters they were editing rather than on the list.
 *
 *  It carries neither a title row nor a footer: the pane header two rows up says
 *  "Filters" while this is up, and the way OUT is the header's own Filter
 *  button, which wears an X while the panel is open (see
 *  `useThreadsHeaderState`). A Close button down here duplicated that exit and
 *  spent a strip of the pane's height on it.
 *
 *  Mounted only while open, and hook-free at its own level so the unit test can
 *  invoke it directly (the nested `ExpandableChannelRow` / `TriCheckbox` use
 *  hooks; this component must not). */
export function ThreadFilterPanel({ onClose }: { onClose: () => void }) {
  const view = drawerView.value;
  const filter = threadChannelFilter.value;
  const triggerChildren = triggerFilterOptions.value;
  const repoChildren = repoFilterOptions.value;
  const appChildren = appFilterOptions.value;
  const selectedTriggers = selectedTriggerIds.value;
  const selectedRepos = selectedRepoIds.value;
  const selectedApps = selectedAppIds.value;
  const includeDeleted = includeDeletedFilterOptions.value;

  const counts: Record<DrawerView, number> = {
    all: 0,
    attention: attentionThreadCount.value,
    review: reviewThreadCount.value,
    running: runningThreadCount.value,
    drafts: draftThreadCount.value,
  };
  // Channels apply only in the default `all` view: the alternate views bypass
  // them. So the whole channel section DIMS under a status view, saying it is
  // not shaping what is on screen. It is never DISABLED, though: the picks are
  // kept and take effect the moment the user takes "All statuses", so a user who
  // arrives on a status view can set up the types they want and go there in one
  // move. Disabling them made that a two-step trip through the `all` view, and
  // made a section the user can legitimately work in refuse the click.
  const channelsDimmed = view !== 'all';
  // "All statuses" and the thread types are ONE choice, not two: you take all
  // statuses and get the ticked types of them. So the checkmark follows the VIEW
  // and stays on that row whatever is ticked below, and the narrowing is
  // reported in words next to the label instead of by moving the mark somewhere
  // else. `threadFilterActive` is the same predicate the closed Filter button
  // highlights on, so the row and the button never disagree.
  const onAllStatuses = view === 'all';
  const typeFilterOn = threadFilterActive.value;
  // "filtered" means what you are being shown differs from ALL of it, whichever
  // setting is doing that: a thread-type selection narrowing the list, or a
  // deleted trigger / repo / app held back from the lists below.
  //
  // Both predicates ask the same question, and it is that one: whether the
  // setting EXCLUDES anything, not whether it sits at its widest. So selecting
  // every channel there is stays neutral (`threadFilterActive`), and on a
  // workspace that has never deleted anything the "Include deleted" switch has
  // nothing to include, so being off hides nothing (`deletedOptionsHidden`).
  //
  // Reported only while the `all` view is the one on screen: it describes what
  // is on screen, and under a status view what is on screen is that status.
  const narrowed = onAllStatuses && (typeFilterOn || deletedOptionsHidden.value);
  // The heading's accent and check track the TYPES, which are what it heads.
  // "Include deleted" sits above it, so accenting for that would point the cue
  // at the wrong section, which is also why the two cues are separate.
  //
  // Not gated on the view, unlike the "filtered" note above: the knobs are live
  // in every view, so the cue reports what is TICKED rather than what is in
  // effect, and under a status view it rides the section's dim, which is what
  // says "set, but not shaping this list".
  const takeAllStatuses = () => { setDrawerView('all'); onClose(); };

  return (
    <div class="thread-filter-panel" role="group" aria-label="Thread filters">
      {/* No title row and no footer: the pane's header carries the title, which
          reads "Filters" while this is up (see ThreadsHeader /
          MobileThreadsHeader), and the same header's Filter button carries the
          way out, wearing an X while the panel is up. So the panel is nothing
          but its own scroll of filters, which is also what lets it wear the
          thread list's own spacing (drawer.css). */}
      {/* The halves are ALTERNATIVES rather than a stack of filters: picking a
          status bypasses the thread-type filter entirely, which is why the
          channel rows dim below. The separator says "or" out loud and the rows
          either side of it are one single-select set, so the whole panel reads
          as a sentence down the page and the relationship is legible before the
          user discovers it by having a section dim on them. */}
      <div class="thread-filter-title" id="thread-filter-status-title">Status</div>
      {/* A radiogroup, not the menu these rows used to claim: they wore
          `menuitemradio`, which is only meaningful inside a `menu`, and the
          anchored dropdown they lived in never set one. Nothing here is a menu
          now (the toggle dropped `aria-haspopup` with the overlay), so the
          single-select list says what it actually is. */}
      <div role="radiogroup" aria-labelledby="thread-filter-status-title">
      {VIEW_META.map(({ view: v, label, Icon }) => {
        const count = counts[v];
        const isActive = v === view;
        return (
          <button
            key={v}
            class={`drawer-view-option${isActive ? ' drawer-view-option-active' : ''}`}
            role="radio"
            aria-checked={isActive}
            // Picking a view applies it and closes the panel: selecting a status
            // is a terminal choice, not a step the user keeps adjusting, and
            // closing is what reveals the list it just filtered.
            onClick={() => { setDrawerView(v); onClose(); }}
          >
            <Icon />
            <span class="drawer-view-label">{label}</span>
            {/* Active marker hugs the label text — a single-select "you are here"
                checkmark sitting right after the name, so it reads as part of the
                text rather than riding the row's trailing edge or mirroring the
                channel rows' leading toggle checkboxes. */}
            {isActive && (
              <span class="drawer-view-check"><CheckIcon /></span>
            )}
            {/* Only "Needs attention" wears the blue `badge`, so the standout
                count lines up with the badge on the threads-header filter icon;
                the other views show a plain muted number. The count is pinned to
                the row's trailing edge (CSS `margin-left:auto`), past the marker. */}
            {count > 0 && (
              <span class={`drawer-view-count${v === 'attention' ? ' badge' : ''}`}>{count}</span>
            )}
          </button>
        );
      })}

      {/* Inside the radiogroup, not between two of them: everything above and
          below this rule is one single-select set, and the rule divides the two
          branches of it rather than separating two independent controls. */}
      <div class="thread-filter-or" role="separator" aria-label="or"><span>or</span></div>

      {/* The fifth and last option: every status, whatever the thread is doing.
          The thread types below are NOT an alternative to it, they NARROW it, so
          it keeps the checkmark whenever the `all` view is on. Terminal like a
          status row, so it closes the panel.

          A `div[role="radio"]` rather than the `<button>` its four siblings use,
          because the "filtered" note carries an <Explainer>, and an explainer is
          a <button>: nesting one inside a button is invalid, and pulling it out
          of the row would either strand the icon at the far edge, away from the
          text it belongs to, or shrink the row's click target to its label.
          A div keeps the whole row clickable and the parens around the icon, at
          the cost of the Enter / Space a button gives free, which is why they
          are handled here. */}
      <div
        class={`drawer-view-option${onAllStatuses ? ' drawer-view-option-active' : ''}`}
        role="radio"
        aria-checked={onAllStatuses}
        tabIndex={0}
        onClick={takeAllStatuses}
        onKeyDown={(e: KeyboardEvent) => {
          // Only the row's OWN keys. The explainer nested in it is a <button>,
          // so its Enter / Space bubbles here, and acting on that would cancel
          // the button's activation and close the panel: the dialog would be
          // unreachable by keyboard. Comparing target to currentTarget covers
          // any future focusable descendant too, since a keydown can only
          // originate on one of those.
          if (e.target !== e.currentTarget) return;
          if (e.key !== 'Enter' && e.key !== ' ') return;
          e.preventDefault();
          takeAllStatuses();
        }}
      >
        <ALL_STATUSES_META.Icon />
        <span class="drawer-view-label">{ALL_STATUSES_META.label}</span>
        {onAllStatuses && (
          <span class="drawer-view-check"><CheckIcon /></span>
        )}
        {narrowed && (
          // The click stops here: the annotation reports on the row, it is not a
          // second way to take it, and the explainer inside would otherwise
          // select the view on its way to opening the dialog.
          <span class="drawer-view-suffix" onClick={(e: Event) => e.stopPropagation()}>
            filtered
            <Explainer title="Filtered">
              <p>
                <strong>All statuses</strong> shows every thread whatever it is doing, and it
                is still showing every status right now. What you are seeing is narrower than
                all of it, though, so the row says so.
              </p>
              {typeFilterOn && (
                <p>
                  Only the thread types ticked under <strong>By thread types</strong> are
                  showing, so threads of the other types are held back. Tick the missing ones
                  back on to widen it again.
                </p>
              )}
              {deletedOptionsHidden.value && (
                <p>
                  <em>Include deleted</em> is off and something deleted exists, so the
                  trigger, repo and app lists below offer only the ones that still exist.
                  Turn it on to pick a trigger, repo or app you have since removed and see
                  the work it did.
                </p>
              )}
            </Explainer>
          </span>
        )}
      </div>

      {/* The modifier for the expandable trigger / repo / app child lists below:
          whether they include entries whose underlying entity is gone (the
          `(deleted)` / `(until …)` rows). Off by default; persisted to
          localStorage. It sits BETWEEN the branch's two rows, so the row naming
          the thread types lands directly on top of them.

          It dims with the section below under a status view and stays fully
          operable there, like every other knob in that section. */}
      <label class={`thread-filter-option${channelsDimmed ? ' thread-filter-option-dimmed' : ''}`}>
        <input
          type="checkbox"
          checked={includeDeleted}
          onChange={() => setIncludeDeletedFilterOptions(!includeDeleted)}
        />
        Include deleted
        <Explainer title="Include deleted">
          <p>
            Expanding <strong>Triggers</strong> or <strong>Coding Agent</strong> lists the
            individual triggers, repos and apps you can filter by. Normally that list only
            offers ones that still exist.
          </p>
          <p>
            Turn this on to list the deleted ones too, marked <em>(deleted)</em>, or{' '}
            <em>(until …)</em> with the date they were last active. Their threads are still
            here: this is how you filter down to the work a trigger did before you removed it.
          </p>
        </Explainer>
      </label>

      </div>

      {/* The channel section's heading. While a type is ticked it takes the
          accent AND a checkmark after the words, the same "something down here
          is on" the row above says as "filtered". Not a selection though: it
          stays a heading with no role, and the single-select set's own mark is
          the one on "All statuses". It dims with the rows it heads when a status
          view bypasses them, keeping the accent under the dim so a selection
          still reads as one. No hairline above it, since a heading already opens
          a section. */}
      <div
        class={`thread-filter-title${typeFilterOn ? ' thread-filter-title-active' : ''}${channelsDimmed ? ' thread-filter-title-dimmed' : ''}`}
        id="thread-filter-types-title"
      >
        By thread types
        {typeFilterOn && (
          <span class="thread-filter-title-check"><CheckIcon /></span>
        )}
      </div>

      {channelsDimmed && (
        <div class="thread-filter-section-note">
          A status shows every thread type. Your picks here apply when you take All statuses.
        </div>
      )}
      {/* A named `role="group"`, not the `<fieldset>` this used to be: the
          fieldset was here for `disabled`, which natively disables every nested
          checkbox, and these knobs are never disabled. A legend-less fieldset
          left behind would carry no accessible name and no purpose, where the
          group points at the heading above for its name. */}
      <div class={`thread-filter-types${channelsDimmed ? ' thread-filter-types-dimmed' : ''}`} role="group" aria-labelledby="thread-filter-types-title">
        {CHANNEL_OPTIONS.map(opt => {
          const icon = channelIcon(opt.value);
          if (opt.value === 'trigger') {
            return (
              <ExpandableChannelRow
                key={opt.value}
                channelOn={filter.has('trigger')}
                label={opt.label}
                icon={icon}
                children={triggerChildren}
                selected={selectedTriggers}
                onToggleChild={toggleTriggerId}
                onToggleChannel={toggleTriggerChannel}
              />
            );
          }
          if (opt.value === CODING_AGENT_CHANNEL) {
            const groups: ChildGroup[] = [];
            if (repoChildren.length > 0) {
              groups.push({ label: 'Repos', items: repoChildren, selected: selectedRepos, onToggleChild: toggleRepoId });
            }
            if (appChildren.length > 0) {
              groups.push({ label: 'Apps', items: appChildren, selected: selectedApps, onToggleChild: toggleAppId });
            }
            return (
              <ExpandableChannelRow
                key={opt.value}
                channelOn={filter.has(CODING_AGENT_CHANNEL)}
                label={opt.label}
                icon={icon}
                groups={groups}
                onToggleChannel={toggleCodingAgentChannel}
              />
            );
          }
          return (
            <label class="thread-filter-option" key={opt.value}>
              <input
                type="checkbox"
                checked={filter.has(opt.value)}
                onChange={() => toggleChannel(opt.value)}
              />
              <span class="thread-filter-channel-icon">{icon}</span>
              {opt.label}
            </label>
          );
        })}
      </div>
    </div>
  );
}

type ExpandableChannelRowProps = {
  channelOn: boolean;
  label: string;
  icon: VNode;
  onToggleChannel: () => void;
} & (
  | { children: ChildOption[]; selected: Set<string>; onToggleChild: (id: string) => void; groups?: undefined }
  | { groups: ChildGroup[]; children?: undefined; selected?: undefined; onToggleChild?: undefined }
);

function ExpandableChannelRow(props: ExpandableChannelRowProps) {
  const { channelOn, label, icon, onToggleChannel } = props;

  // Normalize both shapes into a single `groups` view so the rest of the
  // function doesn't need to discriminate on the union per render. Single
  // groups render their items without the section header (length===1 below).
  const groups: ChildGroup[] = props.groups
    ? props.groups
    : [{
        label: '',
        items: props.children,
        selected: props.selected,
        onToggleChild: props.onToggleChild,
      }];

  // Flatten for the tri-state checkbox math. Lockstep below also keys off the
  // flattened total — a single child across all groups behaves as if there is
  // no per-child choice to make.
  const allItems: ChildOption[] = groups.flatMap(g => g.items);
  const selectionSize = groups.reduce((n, g) => n + g.selected.size, 0);

  // Lockstep: with a single child, "all" and "just this one" are identical
  // results, so parent and child mirror each other and clicking either
  // toggles the channel. The toggle handler is also lockstep-aware (it
  // bypasses the indeterminate-clear early-return) so stale selection from
  // a prior multi-child state doesn't make the click a no-op.
  const lockstep = allItems.length === 1;
  const effectiveSelectedSize = lockstep ? 0 : selectionSize;
  const checked = channelOn && effectiveSelectedSize === 0;
  const indeterminate = channelOn && effectiveSelectedSize > 0;
  const expanded = channelOn && allItems.length > 0;
  const showHeaders = groups.length > 1;

  return (
    <Fragment>
      <label class="thread-filter-option">
        <TriCheckbox
          checked={checked}
          indeterminate={indeterminate}
          onChange={onToggleChannel}
        />
        <span class="thread-filter-channel-icon">{icon}</span>
        {label}
      </label>
      {expanded && groups.map(group => (
        <Fragment key={group.label || 'default'}>
          {/* Its own class, not the change selector's
              `.dropdown-section-header`. That one stands on a dropdown's left
              edge, which here falls left of every row this heading spans. */}
          {showHeaders && (
            <div class="thread-filter-group-title">{group.label}</div>
          )}
          {group.items.map(child => (
            <ChildRow
              key={child.id}
              child={child}
              checked={lockstep ? channelOn : group.selected.has(child.id)}
              onChange={lockstep ? onToggleChannel : () => group.onToggleChild(child.id)}
            />
          ))}
        </Fragment>
      ))}
    </Fragment>
  );
}

function ChildRow({ child, checked, onChange }: { child: ChildOption; checked: boolean; onChange: () => void }) {
  const suffix = child.deleted
    ? (child.lastActivity
        ? `(until ${formatShortDateWithYear(new Date(child.lastActivity))})`
        : '(deleted)')
    : null;
  return (
    <label
      class={`thread-filter-option thread-filter-option-child${child.deleted ? ' thread-filter-option-deleted' : ''}`}
    >
      <input
        type="checkbox"
        checked={checked}
        onChange={onChange}
      />
      <span class="thread-filter-label">{child.label}</span>
      {suffix && <span class="thread-filter-deleted"> {suffix}</span>}
    </label>
  );
}

/** HTML checkboxes only support `indeterminate` via DOM property, so set it
 *  imperatively after render whenever the prop changes. */
function TriCheckbox({ checked, indeterminate, onChange }: { checked: boolean; indeterminate: boolean; onChange: () => void }) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = indeterminate;
  }, [indeterminate]);
  return (
    <input
      ref={ref}
      type="checkbox"
      checked={checked}
      onChange={onChange}
    />
  );
}
