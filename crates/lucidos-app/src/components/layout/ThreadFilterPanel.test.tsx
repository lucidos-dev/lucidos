/**
 * The unified thread Filter panel (ThreadFilterPanel), which renders inside the
 * thread drawer pane: ONE single-select set split by an "or" rule. Above it the
 * four real statuses (Needs attention / Review / Running / Drafts); below it the
 * fifth and last option, "All statuses", then the multi-select channel rows
 * under a "By thread types" heading. Those rows NARROW All statuses rather than
 * competing with it, so the checkmark stays on that row and it grows a
 * "(filtered)" note instead. They are disabled while a non-`all` view is
 * active. These
 * tests invoke the component directly and walk the returned VNode tree WITHOUT
 * descending into the nested function components (<ExpandableChannelRow>,
 * <TriCheckbox>), which use render-time hooks. The component is hook-free at its
 * own level, so direct invocation is safe.
 *
 * The header button's needs-attention badge renders `attentionThreadCount`
 * directly; its semantics (attention-only, excluding review / running) are
 * covered by `components/drawer/attention-view.test.ts`.
 */
import type { ComponentChildren, VNode } from 'preact';
import { beforeEach, describe, expect, it } from 'vitest';
import { ThreadFilterPanel, filterButtonState } from './ThreadFilterPanel';
import {
  ALL_CHANNELS, drawerView, setDrawerView, threadChannelFilter, threadMap, triggers,
  selectedTriggerIds, setSelectedTriggerIds, setSelectedRepoIds, setSelectedAppIds,
  setIncludeDeletedFilterOptions,
} from '../../store/store';
import { FilterIcon, AttentionIcon, ReviewIcon, RunningIcon, DraftsIcon, CloseIcon } from '../shared/icons';
import type { DrawerView } from '../../store/store';
import type { ThreadState, ThreadStatus } from '../../store/thread-events';

type AnyVNode = VNode<Record<string, unknown>>;

function makeThread(id: string, opts: {
  status?: ThreadStatus;
  codingAgentProposed?: boolean;
  /** A trigger thread whose trigger is absent from the registry becomes a
   *  DELETED filter option, which is what `deletedOptionsHidden` keys off. */
  triggerId?: string;
} = {}): ThreadState {
  return {
    meta: {
      id,
      title: id,
      channel: opts.triggerId ? 'trigger' : 'chat',
      triggerId: opts.triggerId,
      initiator: 'user',
      saved: false,
      createdAt: '2026-05-01T00:00:00Z',
      updatedAt: '2026-05-01T00:00:00Z',
      status: opts.status ?? 'idle',
      messageCount: 1,
      section: 'inbox',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0,
      attentionDescendantCount: 0,
      codingAgentHasDiff: false,
      codingAgentProposed: opts.codingAgentProposed ?? false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      lastRevivedAt: '',
      state: 'active',
      latestTodoList: null,
      liveEventWaitCount: 0,
      liveEventWaits: [],
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: false,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

function asMap(threads: ThreadState[]): Map<string, ThreadState> {
  return new Map(threads.map(t => [t.meta.id, t]));
}

/** Collect DOM (string-typed) vnodes matching `cls`, walking arrays + DOM
 *  children. Deliberately does NOT descend into function components, so the
 *  hooks-using <Overlay> / <ExpandableChannelRow> / <TriCheckbox> are never
 *  invoked. */
function findByClass(node: ComponentChildren, cls: string): AnyVNode[] {
  if (node === null || node === undefined || typeof node !== 'object') return [];
  if (Array.isArray(node)) return node.flatMap(n => findByClass(n, cls));
  const v = node as AnyVNode;
  if (typeof v.type !== 'string') return []; // function component — don't invoke
  const out: AnyVNode[] = [];
  const klass = (v.props.class as string | undefined) ?? '';
  if (klass.split(' ').includes(cls)) out.push(v);
  return out.concat(findByClass(v.props.children as ComponentChildren, cls));
}

/** Every DOM `<input>` of a subtree. Same walk as `findByClass`, so it likewise
 *  stops at a function component. */
function findInputs(node: ComponentChildren): AnyVNode[] {
  if (node === null || node === undefined || typeof node !== 'object') return [];
  if (Array.isArray(node)) return node.flatMap(findInputs);
  const v = node as AnyVNode;
  if (typeof v.type !== 'string') return [];
  const out: AnyVNode[] = v.type === 'input' ? [v] : [];
  return out.concat(findInputs(v.props.children as ComponentChildren));
}

/** Plain-text content of a vnode subtree (DOM nodes only). */
function textOf(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(textOf).join('');
  const v = node as AnyVNode;
  if (typeof v.type !== 'string') return '';
  return textOf(v.props.children as ComponentChildren);
}

/** Every labelled row of a subtree, in render order, whichever family it belongs
 *  to. Lets a test assert the reading order across the two row classes, which
 *  interleave under the "or" rule. */
function rowLabelsInOrder(node: ComponentChildren): string[] {
  if (node === null || node === undefined || typeof node !== 'object') return [];
  if (Array.isArray(node)) return node.flatMap(rowLabelsInOrder);
  const v = node as AnyVNode;
  if (typeof v.type !== 'string') return [];
  const klass = ((v.props.class as string | undefined) ?? '').split(' ');
  if (klass.includes('drawer-view-option') || klass.includes('thread-filter-option')) {
    return [textOf(v)];
  }
  return rowLabelsInOrder(v.props.children as ComponentChildren);
}

/** The single-select set: the four status rows, the "or" rule, "All statuses"
 *  and the "Include deleted" modifier all live in one `role="radiogroup"`. */
function findRadiogroup(node: ComponentChildren): AnyVNode | undefined {
  if (node === null || node === undefined || typeof node !== 'object') return undefined;
  if (Array.isArray(node)) {
    for (const n of node) {
      const hit = findRadiogroup(n);
      if (hit) return hit;
    }
    return undefined;
  }
  const v = node as AnyVNode;
  if (typeof v.type !== 'string') return undefined;
  if (v.props.role === 'radiogroup') return v;
  return findRadiogroup(v.props.children as ComponentChildren);
}

function render(onClose: () => void = () => {}) {
  // The panel is hook-free at its own level (no useRef/useState/useEffect at the
  // top; the Escape registration lives in the store with the open state), so it
  // can be invoked directly. It returns the panel div, whose children hold the
  // Status heading, the one radiogroup, and the channel group.
  const tree = ThreadFilterPanel({ onClose }) as AnyVNode;
  const children = tree.props.children as ComponentChildren;
  const options = findByClass(children, 'drawer-view-option');
  const typesGroup = findByClass(children, 'thread-filter-types')[0];
  const radiogroup = findRadiogroup(children);
  // By the row's LABEL span, not its whole text: a row also carries its count
  // and, on All statuses, the "filtered" suffix.
  const rowNamed = (label: string) =>
    options.find(o => findByClass(o, 'drawer-view-label').map(textOf).includes(label))!;
  return { tree, children, options, typesGroup, radiogroup, rowNamed };
}

beforeEach(() => {
  threadMap.value = new Map();
  // Loaded-but-empty: the option lists return [] until the registry loads, so a
  // test about DELETED options has to get past that gate.
  triggers.value = { status: 'loaded', data: [] };
  setDrawerView('all');
  // Every channel selected is the neutral, unfiltered state, so
  // `threadFilterActive` is false, so "All statuses" carries no "(filtered)"
  // note and the "By thread types" heading no accent. localStorage may have
  // restored something narrower.
  threadChannelFilter.value = new Set(ALL_CHANNELS);
  setSelectedTriggerIds(new Set());
  setSelectedRepoIds(new Set());
  setSelectedAppIds(new Set());
  // Include deleted ON is the widest setting, and the panel counts it as a
  // filter when off (which is the product default), so the tests need it on to
  // have an unnarrowed baseline to compare against.
  setIncludeDeletedFilterOptions(true);
});

describe('ThreadFilterPanel: shape', () => {
  it('is a plain panel element, NOT an <Overlay>', () => {
    // It lives inside the thread drawer pane, so it must not carry the
    // dismiss-and-swallow contract: a click on the thread or content pane while
    // it is open belongs to whatever the user clicked.
    const { tree } = render();
    expect(typeof tree.type).toBe('string');
    expect(tree.props.class).toBe('thread-filter-panel');
  });

  it('carries two headings and one "or" rule, and the rule sits INSIDE the radiogroup', () => {
    // Everything either side of the rule is one single-select set, so the rule
    // divides the two halves of one control rather than separating two controls.
    // Both sections are headed, and neither heading is an option.
    const { children, radiogroup } = render();
    expect(findByClass(children, 'thread-filter-title').map(textOf))
      .toEqual(['Status', 'By thread types']);
    expect(findByClass(children, 'thread-filter-or').map(textOf)).toEqual(['or']);
    expect(findByClass(radiogroup, 'thread-filter-or')).toHaveLength(1);
  });

  it('says why the thread-type section is dim, and what its knobs still do', () => {
    // Both halves matter: the section is bypassed HERE, and the picks made in it
    // are not thrown away, they are waiting for "All statuses". Without the
    // second sentence a dim section reads as an unavailable one.
    setDrawerView('review');
    const note = findByClass(render().children, 'thread-filter-section-note')[0];
    expect(textOf(note)).toBe(
      'A status shows every thread type. Your picks here apply when you take All statuses.',
    );
  });

  it('exposes the Status rows as a radiogroup, not as menu items', () => {
    // They wore `menuitemradio`, which means nothing outside a `menu`, and no
    // ancestor ever declared one. Nothing here is a menu now.
    const { options } = render();
    expect(options.map(o => o.props.role)).toEqual(Array(5).fill('radio'));
  });

  it('carries no title row and no footer: the header Filter button is both ends', () => {
    // The pane header above says "Filters" while the panel is up, so a header
    // inside the panel would repeat the one two rows above it, and the same
    // header's Filter button is the way out (it wears an X while the panel is
    // open, see `filterButtonState`). A Close footer down here duplicated that
    // exit and spent a strip of the pane's height on it.
    const { children } = render();
    expect(findByClass(children, 'thread-filter-panel-header')).toHaveLength(0);
    expect(findByClass(children, 'thread-filter-panel-footer')).toHaveLength(0);
    expect(findByClass(children, 'thread-filter-close')).toHaveLength(0);
  });

  it('is nothing but its own filters: no wrapper between the panel and them', () => {
    // With nothing docked above or below, the panel IS the pane's scroller, which
    // is what lets it wear the thread list's own padding (drawer.css). An inner
    // scrolling body existed only to keep the footer off the last row.
    const { children } = render();
    expect(findByClass(children, 'thread-filter-panel-body')).toHaveLength(0);
    expect(findByClass(children, 'thread-filter-types')).toHaveLength(1);
    expect(findByClass(children, 'drawer-view-option')).toHaveLength(5);
  });
});

describe('ThreadFilterPanel: View section', () => {
  it('lists the four real statuses, then the two `all` rows under the rule', () => {
    // "All statuses" is the absence of a status rather than one of them, so it
    // is not in the Status list: it sits under the "or" with the row that
    // narrows it, and the two of them own the channel rows below.
    const { options } = render();
    expect(options.map(o => textOf(o))).toEqual([
      'Needs attention', 'Review', 'Running', 'Drafts', 'All statuses',
    ]);
  });

  it('marks exactly the active view as checked', () => {
    setDrawerView('drafts');
    const checked = render().options.filter(o => o.props['aria-checked'] === true);
    expect(checked).toHaveLength(1);
    expect(textOf(checked[0])).toBe('Drafts');
  });

  it('selecting an option switches the drawer view', () => {
    const review = render().options.find(o => textOf(o) === 'Review')!;
    (review.props.onClick as () => void)();
    expect(drawerView.value).toBe('review');
  });

  it('selecting a view also closes the panel', () => {
    // A status is a terminal choice, and closing is what reveals the list it
    // just filtered (the panel covers the whole pane).
    let closed = 0;
    const review = render(() => { closed++; }).options.find(o => textOf(o) === 'Review')!;
    (review.props.onClick as () => void)();
    expect(closed).toBe(1);
  });

  it('selecting Running switches the drawer view', () => {
    const running = render().options.find(o => textOf(o) === 'Running')!;
    (running.props.onClick as () => void)();
    expect(drawerView.value).toBe('running');
  });

  it('per-view counts render on the option rows', () => {
    threadMap.value = asMap([
      makeThread('waiting', { status: 'waiting_for_user_answer' }),
      makeThread('proposed', { codingAgentProposed: true }),
      makeThread('running', { status: 'running' }),
    ]);
    const { options } = render();
    const attention = options.find(o => textOf(o).startsWith('Needs attention'))!;
    const review = options.find(o => textOf(o).startsWith('Review'))!;
    const running = options.find(o => textOf(o).startsWith('Running'))!;
    expect(findByClass(attention, 'drawer-view-count').map(textOf)).toEqual(['1']);
    expect(findByClass(review, 'drawer-view-count').map(textOf)).toEqual(['1']);
    expect(findByClass(running, 'drawer-view-count').map(textOf)).toEqual(['1']);
  });

  it('only "Needs attention" wears the blue badge — the others show a plain number', () => {
    threadMap.value = asMap([
      makeThread('waiting', { status: 'waiting_for_user_answer' }),
      makeThread('proposed', { codingAgentProposed: true }),
      makeThread('running', { status: 'running' }),
    ]);
    const { options } = render();
    const attention = options.find(o => textOf(o).startsWith('Needs attention'))!;
    const review = options.find(o => textOf(o).startsWith('Review'))!;
    const running = options.find(o => textOf(o).startsWith('Running'))!;
    // The attention count carries `badge` (blue pill); the others carry only
    // the plain `drawer-view-count` class.
    expect(findByClass(attention, 'badge')).toHaveLength(1);
    expect(findByClass(review, 'badge')).toHaveLength(0);
    expect(findByClass(running, 'badge')).toHaveLength(0);
  });
});

describe('ThreadFilterPanel: All statuses and the types that narrow it', () => {
  const ALL = 'All statuses';
  const suffixes = () => findByClass(render().children, 'drawer-view-suffix').map(textOf);
  const typesHeading = () => findByClass(render().children, 'thread-filter-title')[1];

  it('keeps the checkmark on "All statuses" whatever is ticked below', () => {
    // The thread types NARROW that view rather than competing with it, so the
    // mark follows `drawerView` and never moves off the row.
    expect(render().rowNamed(ALL).props['aria-checked']).toBe(true);
    threadChannelFilter.value = new Set(['chat']);
    expect(render().rowNamed(ALL).props['aria-checked']).toBe(true);
  });

  it('says "filtered" for a thread-type selection', () => {
    // What is being shown differs from all of it, which is the whole rule.
    expect(suffixes()).toEqual([]);
    threadChannelFilter.value = new Set(['chat']);
    expect(suffixes()).toEqual(['filtered']);
  });

  it('says "filtered" only when Include deleted actually HIDES something', () => {
    // The switch being off is not enough. On a workspace that has never deleted
    // a trigger, repo or app it excludes nothing, so the row stays quiet: what
    // decides is the difference between what is offered and what exists.
    setIncludeDeletedFilterOptions(false);
    expect(suffixes()).toEqual([]);

    // A trigger thread whose trigger is gone from the registry is a deleted
    // option, and with the switch off it is held back.
    threadMap.value = asMap([makeThread('t1', { triggerId: 'gone' })]);
    expect(suffixes()).toEqual(['filtered']);

    setIncludeDeletedFilterOptions(true);
    expect(suffixes()).toEqual([]);
  });

  it('carries the explainer in the note itself, with no parentheses', () => {
    // Which is the whole reason the row is a `div[role="radio"]`: an explainer
    // is a <button>, and a button cannot nest inside another one.
    threadChannelFilter.value = new Set(['chat']);
    const suffix = findByClass(render().children, 'drawer-view-suffix')[0];
    const kids = (suffix.props.children as unknown[]).filter(Boolean);
    // The word, then the <Explainer>. No parens around either.
    expect(kids).toHaveLength(2);
    expect(kids[0]).toBe('filtered');
    expect(typeof (kids[1] as AnyVNode).type).toBe('function');
  });

  it('is a div that answers Enter and Space, since it is no longer a button', () => {
    let closed = 0;
    const row = render(() => { closed++; }).rowNamed(ALL);
    expect(row.type).toBe('div');
    expect(row.props.tabIndex).toBe(0);
    setDrawerView('review');
    let prevented = 0;
    const self = {};
    const press = (key: string) => (row.props.onKeyDown as (e: KeyboardEvent) => void)(
      { key, target: self, currentTarget: self, preventDefault: () => { prevented++; } } as unknown as KeyboardEvent,
    );
    press('Enter');
    expect(drawerView.value).toBe('all');
    expect(prevented).toBe(1);
    // Anything else is left to the browser.
    press('a');
    expect(prevented).toBe(1);
  });

  it('leaves a key pressed on the explainer INSIDE it alone', () => {
    // The explainer is a <button> nested in this radio, so its own Enter /
    // Space bubbles up here. Acting on that would cancel the button's
    // activation and close the panel, leaving the dialog unreachable by
    // keyboard, which is the whole hazard of hand-rolling a button's keys.
    setIncludeDeletedFilterOptions(false);
    threadMap.value = asMap([makeThread('t1', { triggerId: 'gone' })]);
    setDrawerView('review');
    let closed = 0;
    let prevented = 0;
    const row = render(() => { closed++; }).rowNamed(ALL);
    (row.props.onKeyDown as (e: KeyboardEvent) => void)({
      key: 'Enter',
      target: {},          // the explainer's button
      currentTarget: {},   // the row
      preventDefault: () => { prevented++; },
    } as unknown as KeyboardEvent);
    expect(drawerView.value).toBe('review');
    expect(closed).toBe(0);
    expect(prevented).toBe(0);
  });

  it('says nothing about filtering while a STATUS view owns the list', () => {
    // This row's note reports on what is on screen, and what is on screen under
    // a status view is that status: it is not "all statuses, narrowed", so the
    // row that says so must stay quiet.
    //
    // The heading below is deliberately NOT gated the same way (see the dim test
    // in the section above): its cues report what is TICKED, and the types stay
    // pickable in every view, so they keep saying so under the section's dim.
    threadChannelFilter.value = new Set(['chat']);
    setIncludeDeletedFilterOptions(false);
    threadMap.value = asMap([makeThread('t1', { triggerId: 'gone' })]);
    setDrawerView('running');
    expect(suffixes()).toEqual([]);
  });

  it('offers the explainer only while it is filtered', () => {
    // Nothing to explain in the plain everything view, and an icon there would
    // be permanent chrome on the row's most common state.
    expect(findByClass(render().children, 'drawer-view-suffix')).toHaveLength(0);
    threadChannelFilter.value = new Set(['chat']);
    expect(findByClass(render().children, 'drawer-view-suffix')).toHaveLength(1);
  });

  it('loses the checkmark only when a real status takes it', () => {
    threadChannelFilter.value = new Set(['chat']);
    setDrawerView('review');
    const { rowNamed, options } = render();
    expect(rowNamed(ALL).props['aria-checked']).toBe(false);
    expect(options.filter(o => o.props['aria-checked'] === true).map(textOf)).toEqual(['Review']);
  });

  it('takes the view and closes, KEEPING the thread types it is filtered by', () => {
    // One choice, not two: picking all statuses does not throw away the types
    // narrowing them, or the "(filtered)" state would be unreachable by tapping
    // the row it describes.
    threadChannelFilter.value = new Set(['chat']);
    setSelectedTriggerIds(new Set(['trigger-1']));
    setDrawerView('review');
    let closed = 0;
    const { rowNamed } = render(() => { closed++; });
    (rowNamed(ALL).props.onClick as () => void)();
    expect(drawerView.value).toBe('all');
    expect(threadChannelFilter.value).toEqual(new Set(['chat']));
    expect(selectedTriggerIds.value).toEqual(new Set(['trigger-1']));
    expect(closed).toBe(1);
  });

  it('accents AND checks the "By thread types" heading, for the TYPES only', () => {
    // The heading echoes what is happening under IT, in both cues at once.
    // "Include deleted" sits above it, so either cue for that would point at
    // the wrong section, even though the row's "filtered" note counts both.
    const heading = () => typesHeading().props.class as string;
    const headingChecks = () =>
      findByClass(render().children, 'thread-filter-title-check').length;
    expect(heading()).not.toContain('thread-filter-title-active');
    expect(headingChecks()).toBe(0);

    setIncludeDeletedFilterOptions(false);
    threadMap.value = asMap([makeThread('t1', { triggerId: 'gone' })]);
    expect(heading()).not.toContain('thread-filter-title-active');
    expect(headingChecks()).toBe(0);

    threadChannelFilter.value = new Set(['chat']);
    expect(heading()).toContain('thread-filter-title-active');
    expect(headingChecks()).toBe(1);
    // Still a heading, not an option: no role, and the single-select set's own
    // mark stays on "All statuses" (a different class).
    expect(typesHeading().props.role).toBeUndefined();
    expect(findByClass(render().children, 'drawer-view-check')).toHaveLength(1);
  });

  it('dims that heading under a status view, keeping the cues UNDER the dim', () => {
    // A bypassed section reads dim whatever is ticked inside it, but the types
    // stay pickable there and a ticked one still has to read as ticked. So the
    // dim rides alongside the accent and the check rather than replacing them
    // (`.thread-filter-title-dimmed` is opacity only).
    threadChannelFilter.value = new Set(['chat']);
    setDrawerView('review');
    expect(typesHeading().props.class).toContain('thread-filter-title-dimmed');
    expect(typesHeading().props.class).toContain('thread-filter-title-active');
    expect(findByClass(render().children, 'thread-filter-title-check')).toHaveLength(1);
  });

  it('opens its section with no hairline above it', () => {
    // A heading already opens a section, so the divider that used to close off
    // "Include deleted" is gone with it.
    expect(findByClass(render().children, 'thread-filter-divider')).toHaveLength(0);
  });
});

describe('ThreadFilterPanel: Show (channel) section', () => {
  it('is undimmed in the default All view', () => {
    setDrawerView('all');
    expect(render().typesGroup.props.class).not.toContain('thread-filter-types-dimmed');
  });

  it('dims when a non-All view is active', () => {
    // The alternate views bypass the channel filter, so the whole section says
    // so in place rather than being hidden.
    setDrawerView('review');
    expect(render().typesGroup.props.class).toContain('thread-filter-types-dimmed');
  });

  it('is NEVER disabled: the knobs stay live in every view', () => {
    // The dim reports that this section is not shaping the list on screen. It
    // must not also take the section away: a user on a status view sets the
    // types they want here and takes "All statuses" in one move. So no
    // `disabled` anywhere, on the group or on any control inside it, and no
    // `<fieldset>` (whose only job here was to disable them all natively, which
    // is also how the expandable rows' own checkboxes used to go dead: nothing
    // hands them a `disabled` of their own, so removing the fieldset is what
    // makes the whole section live).
    for (const v of ['all', 'attention', 'review', 'running', 'drafts'] as const) {
      setDrawerView(v);
      const group = render().typesGroup;
      expect(group.type).toBe('div');
      expect(group.props.disabled).toBeUndefined();
      const boxes = findInputs(group);
      expect(boxes.length).toBeGreaterThan(0);
      for (const box of boxes) expect(box.props.disabled).toBeFalsy();
    }
  });

  it('is a named group, so the heading above names it', () => {
    // The `<fieldset>` it replaced carried no `<legend>`, so it had no
    // accessible name at all; the group borrows the heading's.
    const group = render().typesGroup;
    expect(group.props.role).toBe('group');
    expect(group.props['aria-labelledby']).toBe('thread-filter-types-title');
    const heading = findByClass(render().children, 'thread-filter-title')
      .find(h => textOf(h).startsWith('By thread types'))!;
    expect(heading.props.id).toBe('thread-filter-types-title');
  });

  it('holds only the channel rows: "Include deleted" is not in the group', () => {
    // The modifier moved up between the branch's two rows, so the phrase
    // the "By thread types" heading lands directly on the types it names. That
    // puts it inside the radiogroup, where a nested grouping element has no
    // business.
    const rows = findByClass(render().typesGroup, 'thread-filter-option');
    expect(rows.map(textOf)).not.toContain('Include deleted');
  });
});

describe('ThreadFilterPanel: Include deleted', () => {
  const includeDeletedRow = () =>
    findByClass(render().radiogroup, 'thread-filter-option')
      .find(r => textOf(r).startsWith('Include deleted'))!;

  it('sits between the two `all` rows, so the branch row lands on the types', () => {
    // Reading order under the rule: All statuses, then Include deleted, with
    // the "By thread types" heading (outside this group) pressed right up
    // against the thread types it names.
    expect(rowLabelsInOrder(render().radiogroup)).toEqual([
      'Needs attention', 'Review', 'Running', 'Drafts',
      'All statuses', 'Include deleted',
    ]);
  });

  it('dims under a status view but stays operable, like the section it matches', () => {
    // It sits above the group rather than inside it, so it carries its own dim
    // to match. Opacity only, and no `disabled`: it is one of the knobs the user
    // may set here and have applied on taking "All statuses".
    setDrawerView('review');
    const row = includeDeletedRow();
    expect(row.props.class).toContain('thread-filter-option-dimmed');
    expect(findInputs(row)[0].props.disabled).toBeFalsy();
  });

  it('is undimmed in the default all-statuses view', () => {
    const row = includeDeletedRow();
    expect(row.props.class).not.toContain('thread-filter-option-dimmed');
  });
});

describe('filterButtonState', () => {
  const shut = (over: Partial<Parameters<typeof filterButtonState>[0]> = {}) => filterButtonState({
    view: 'all', panelOpen: false, channelFilterActive: false, attentionCount: 0, ...over,
  });

  // Closed, the threads-header Filter button reflects the selected view: the
  // funnel for the default `all`, each view's own glyph otherwise.
  it('wears the funnel for all and the view glyph otherwise', () => {
    expect(shut().Icon).toBe(FilterIcon);
    expect(shut({ view: 'attention' }).Icon).toBe(AttentionIcon);
    expect(shut({ view: 'review' }).Icon).toBe(ReviewIcon);
    expect(shut({ view: 'running' }).Icon).toBe(RunningIcon);
    expect(shut({ view: 'drafts' }).Icon).toBe(DraftsIcon);
  });

  it('falls back to the All statuses funnel for a view it does not recognize', () => {
    // The fallback resolves the `all` entry BY NAME. It used to take
    // `VIEW_META[0]`, which silently became "Needs attention" the moment All
    // statuses moved to the foot of the list.
    expect(shut({ view: 'nope' as DrawerView }).Icon).toBe(FilterIcon);
  });

  it('is highlighted when a status view or a channel filter is on, and reports the attention count', () => {
    expect(shut().active).toBe(false);
    expect(shut({ view: 'review' }).active).toBe(true);
    expect(shut({ channelFilterActive: true }).active).toBe(true);
    expect(shut({ attentionCount: 3 }).badge).toBe(3);
  });

  // Open, the button stops reporting and offers the way out: an X, no
  // highlight, no badge. The panel underneath is already saying what the filter
  // is, so repeating it over the exit glyph only crowds it.
  it('drops to a bare X while the panel is open, whatever the filter state', () => {
    for (const view of ['all', 'attention', 'review', 'running', 'drafts'] as const) {
      const open = filterButtonState({ view, panelOpen: true, channelFilterActive: true, attentionCount: 4 });
      expect(open.Icon).toBe(CloseIcon);
      expect(open.active).toBe(false);
      expect(open.badge).toBe(0);
    }
  });
});
