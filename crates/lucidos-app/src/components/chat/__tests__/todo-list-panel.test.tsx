import { describe, it, expect } from 'vitest';
import type { ComponentChildren, ComponentType, VNode } from 'preact';
import { todoListIndicatorBody, todoListPanelBody } from '../TodoListPanel';
import { TodoListIcon } from '../../shared/icons';
import type { TodoItem } from '../../../store/thread-events';

/** Flatten a vnode tree into HTML-ish text preserving class, data-* attrs,
 *  and aria-* attrs so we can assert on per-state styling. Same pattern as
 *  directory-picker-loadable.test.tsx. */
function vnodeToText(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(vnodeToText).join('');
  const v = node as VNode<Record<string, unknown> & { children?: ComponentChildren }>;
  const tag = typeof v.type === 'string' ? v.type : '';
  const attrs: string[] = [];
  for (const [k, val] of Object.entries(v.props ?? {})) {
    if (k === 'children') continue;
    if (k.startsWith('on')) continue;
    if (val === undefined || val === null || val === false) continue;
    attrs.push(` ${k}="${val}"`);
  }
  const inner = vnodeToText(v.props?.children);
  return tag ? `<${tag}${attrs.join('')}>${inner}</${tag}>` : inner;
}

/** The component vnodes in a tree, in render order. `vnodeToText` can't see
 *  inside one (it flattens host elements only), and the glyph we care about
 *  IS a component, so this is how the icon identity is asserted. */
function componentTypes(node: ComponentChildren): ComponentType[] {
  if (node === null || node === undefined || typeof node !== 'object') return [];
  if (Array.isArray(node)) return node.flatMap(componentTypes);
  const v = node as VNode<{ children?: ComponentChildren }>;
  const self = typeof v.type === 'function' ? [v.type as ComponentType] : [];
  return [...self, ...componentTypes(v.props?.children)];
}

const NOOP = () => {};

// ──────────────────────────────────────────────────────────────────────────
// Indicator: hidden when no items, otherwise ONE ticked-checkbox SVG icon (no
// count) whatever the state. The SVG is pixel-identical to the adjacent
// ImageIcon because both inherit `.icon-btn.header-icon svg` sizing
// (--icon-size-lg). There are TWO states, carried by `data-state` and rendered
// as COLOR in todo-list.css, never as a different glyph: `in-progress` is
// accent, `idle` is the row's gray. Counts, and the words `waiting` and
// `abandoned`, go into the tooltip and aria-label instead. Tap target opens the
// panel.
// ──────────────────────────────────────────────────────────────────────────

describe('todoListIndicatorBody', () => {
  it('renders nothing when items is null (never written)', () => {
    expect(todoListIndicatorBody({ items: null, onClick: NOOP })).toBeNull();
  });

  it('renders nothing when items is empty (explicitly cleared)', () => {
    expect(todoListIndicatorBody({ items: [], onClick: NOOP })).toBeNull();
  });

  // *Todo notes* can outlive their items. Under ADR 0085's context mode an
  // agent that finished its plan still writes `todos: []` with a pointer worth
  // keeping. Hiding the indicator there would make the block the whole mode
  // rests on the one thing the user cannot see.
  it('still shows the indicator for a cleared list that kept notes', () => {
    const text = vnodeToText(
      todoListIndicatorBody({ items: [], notes: 'the report is at artifacts/week.md', onClick: NOOP }),
    );
    expect(text).toContain('data-role="todo-indicator"');
    expect(text).toContain('data-state="idle"');
    expect(text).toContain('data-tooltip="Notes kept"');
    expect(text).toContain('aria-label="Todo list: no items, notes kept. Click to expand."');
  });

  it('renders the in-progress state when an item is mid-flight, with the active form in the tooltip', () => {
    const items: TodoItem[] = [
      { content: 'a', active_form: 'doing a', status: 'completed' },
      { content: 'b', active_form: 'doing b', status: 'in_progress' },
      { content: 'c', active_form: 'doing c', status: 'pending' },
    ];
    const text = vnodeToText(todoListIndicatorBody({ items, onClick: NOOP }));
    expect(text).toContain('data-role="todo-indicator"');
    expect(text).toContain('data-state="in-progress"');
    expect(text).not.toContain('data-state="idle"');
    // Counts moved off the glyph and into the aria-label.
    expect(text).not.toContain('1/3');
    expect(text).toContain('1 of 3 done');
    expect(text).toContain('data-tooltip="doing b"');
    // The aria-label NAMES the in-progress state, it does not just count. Both
    // states render the same glyph and differ only in colour. A screen reader
    // can't read colour, forced-colors mode overwrites it, and the tooltip is
    // desktop-hover only. So this is the one non-visual channel that tells idle
    // and in-progress apart.
    expect(text).toContain('aria-label="Todo list: doing b. 1 of 3 done. Click to expand."');
  });

  it('renders the idle state when no item is in progress', () => {
    const items: TodoItem[] = [
      { content: 'a', active_form: 'doing a', status: 'pending' },
      { content: 'b', active_form: 'doing b', status: 'pending' },
    ];
    const text = vnodeToText(todoListIndicatorBody({ items, onClick: NOOP }));
    expect(text).toContain('data-state="idle"');
    expect(text).not.toContain('0/2');
    expect(text).toContain('0 of 2 done');
  });

  it('renders the idle state when every item is completed', () => {
    const items: TodoItem[] = [
      { content: 'a', active_form: 'doing a', status: 'completed' },
      { content: 'b', active_form: 'doing b', status: 'completed' },
    ];
    const text = vnodeToText(todoListIndicatorBody({ items, onClick: NOOP }));
    expect(text).toContain('data-state="idle"');
    expect(text).not.toContain('2/2');
    expect(text).toContain('2 of 2 done');
  });

  it('marks the button as a row item so the prompt-row overflow detector counts its width', () => {
    const items: TodoItem[] = [
      { content: 'a', active_form: 'doing a', status: 'pending' },
    ];
    const text = vnodeToText(todoListIndicatorBody({ items, onClick: NOOP }));
    expect(text).toContain('data-row-item');
  });

  it('stays idle for abandoned items, and says so in words', () => {
    // Abandoned used to dim the glyph. A dim reads as a disabled button, for a
    // fact that is history rather than activity, so the count carries it alone.
    const items: TodoItem[] = [
      { content: 'a', active_form: 'doing a', status: 'completed' },
      { content: 'b', active_form: 'doing b', status: 'abandoned' },
      { content: 'c', active_form: 'doing c', status: 'abandoned' },
    ];
    const text = vnodeToText(todoListIndicatorBody({ items, onClick: NOOP }));
    expect(text).toContain('data-state="idle"');
    expect(text).not.toContain('data-state="in-progress"');
    expect(text).toContain('1 of 3 done, 2 abandoned');
  });

  it('stays idle for items parked on a live event wait, and says so in words', () => {
    // Waiting used to pulse the glyph gray. An item is `waiting` only while the
    // thread holds a live event wait. That is exactly when the *waiting
    // indicator* renders in accent beside this button, so the pulse repeated a
    // signal the row already carried.
    const items: TodoItem[] = [
      { content: 'a', active_form: 'doing a', status: 'completed' },
      { content: 'b', active_form: 'doing b', status: 'waiting' },
      { content: 'c', active_form: 'doing c', status: 'waiting' },
    ];
    const text = vnodeToText(todoListIndicatorBody({ items, onClick: NOOP }));
    expect(text).toContain('data-state="idle"');
    expect(text).not.toContain('data-state="in-progress"');
    expect(text).toContain('1 of 3 done, 2 waiting');
    expect(text).toContain(
      'aria-label="Todo list: 1 of 3 done, 2 waiting. Click to expand."',
    );
  });

  it('says waiting rather than abandoned, because waiting is the live fact', () => {
    // The two no longer differ in paint, so the precedence lives entirely in
    // the words: a list carrying both has parked items still going somewhere.
    const items: TodoItem[] = [
      { content: 'a', active_form: 'doing a', status: 'abandoned' },
      { content: 'b', active_form: 'doing b', status: 'waiting' },
    ];
    const text = vnodeToText(todoListIndicatorBody({ items, onClick: NOOP }));
    expect(text).toContain('0 of 2 done, 1 waiting');
    expect(text).not.toContain('abandoned');
  });

  it('is lit for in-progress and gray for every other list, and never a third state', () => {
    // The whole glanceable contract: accent means an item is being worked right
    // now, which is the composer row's own language. Anything else is idle.
    const byState: Record<string, TodoItem['status'][]> = {
      'in-progress': ['in_progress'],
      idle: ['pending', 'completed', 'waiting', 'abandoned'],
    };
    for (const [state, statuses] of Object.entries(byState)) {
      for (const status of statuses) {
        const items: TodoItem[] = [{ content: 'a', active_form: 'doing a', status }];
        expect(vnodeToText(todoListIndicatorBody({ items, onClick: NOOP })))
          .toContain(`data-state="${state}"`);
      }
    }
  });

  it('renders the SAME ticked-checkbox glyph in both states, so only its painting differs', () => {
    // The state must never switch the shape. The pair this test was written
    // against drew this same checkbox for idle and a filled dome inside a
    // checkbox for in-progress, and the second one read as nothing
    // recognizable at 1.25rem. Whatever the agent is doing, the button has to
    // keep saying "todo list".
    const statuses: TodoItem['status'][] = ['pending', 'in_progress', 'waiting', 'abandoned'];
    for (const status of statuses) {
      const items: TodoItem[] = [{ content: 'a', active_form: 'doing a', status }];
      expect(componentTypes(todoListIndicatorBody({ items, onClick: NOOP })))
        .toEqual([TodoListIcon]);
    }
  });

  it('keeps the in-progress state when there are also abandoned items', () => {
    // Mixed state can briefly happen on stale UI snapshots between events;
    // the in-progress signal wins because the agent is actively working.
    const items: TodoItem[] = [
      { content: 'a', active_form: 'doing a', status: 'completed' },
      { content: 'b', active_form: 'doing b', status: 'in_progress' },
      { content: 'c', active_form: 'doing c', status: 'abandoned' },
    ];
    const text = vnodeToText(todoListIndicatorBody({ items, onClick: NOOP }));
    expect(text).toContain('data-state="in-progress"');
    expect(text).toContain('data-tooltip="doing b"');
  });
});

// ──────────────────────────────────────────────────────────────────────────
// Panel — renders the full list. in_progress rows show active_form;
// pending/completed rows show content. One row per item.
// ──────────────────────────────────────────────────────────────────────────

describe('todoListPanelBody', () => {
  it('renders one row per item with content for non-in_progress rows', () => {
    const items: TodoItem[] = [
      { content: 'Run tests', active_form: 'Running tests', status: 'pending' },
      { content: 'Write docs', active_form: 'Writing docs', status: 'completed' },
    ];
    const text = vnodeToText(todoListPanelBody({ items, onClose: NOOP }));
    // The body renders the panel's CONTENTS; the `.todo-panel` box itself is
    // the <Overlay> panel, which is what `useAnchoredPosition` positions. So
    // the list container is the identity marker here, not the panel's role.
    expect(text).toContain('todo-panel-list');
    expect(text).toContain('Run tests');
    expect(text).toContain('Write docs');
    // The active_form variants must NOT leak for pending/completed rows.
    expect(text).not.toContain('Running tests');
    expect(text).not.toContain('Writing docs');
  });

  it('renders active_form (not content) for the in_progress row', () => {
    const items: TodoItem[] = [
      { content: 'Run tests', active_form: 'Running tests', status: 'in_progress' },
    ];
    const text = vnodeToText(todoListPanelBody({ items, onClose: NOOP }));
    expect(text).toContain('Running tests');
    expect(text).not.toContain('>Run tests<');
  });

  it('stamps status as data-status on each row so CSS can branch on it', () => {
    const items: TodoItem[] = [
      { content: 'a', active_form: 'doing a', status: 'pending' },
      { content: 'b', active_form: 'doing b', status: 'in_progress' },
      { content: 'c', active_form: 'doing c', status: 'completed' },
    ];
    const text = vnodeToText(todoListPanelBody({ items, onClose: NOOP }));
    expect(text).toContain('data-status="pending"');
    expect(text).toContain('data-status="in_progress"');
    expect(text).toContain('data-status="completed"');
  });

  it('renders an empty panel (no rows) when items is empty — used as the cleared state', () => {
    const text = vnodeToText(todoListPanelBody({ items: [], onClose: NOOP }));
    expect(text).toContain('todo-panel-list');
    expect(text).not.toContain('data-status=');
  });

  it('renders abandoned rows with the content (not active_form) and an abandoned tag so they are clearly distinguished', () => {
    const items: TodoItem[] = [
      { content: 'Run tests', active_form: 'Running tests', status: 'abandoned' },
    ];
    const text = vnodeToText(todoListPanelBody({ items, onClose: NOOP }));
    expect(text).toContain('data-status="abandoned"');
    // Abandoned rows show `content`, never the present-continuous form —
    // "Running tests" would imply the agent is still working it.
    expect(text).toContain('>Run tests<');
    expect(text).not.toContain('>Running tests<');
    expect(text).toContain('todo-panel-status-tag');
    expect(text).toContain('>abandoned<');
  });

  it('renders waiting rows with the content and a waiting tag, so a parked item is not read as dropped', () => {
    const items: TodoItem[] = [
      { content: 'Run tests', active_form: 'Running tests', status: 'waiting' },
    ];
    const text = vnodeToText(todoListPanelBody({ items, onClose: NOOP }));
    expect(text).toContain('data-status="waiting"');
    // Same reason as abandoned: nothing is running, so the present-continuous
    // form would claim activity that stopped.
    expect(text).toContain('>Run tests<');
    expect(text).not.toContain('>Running tests<');
    expect(text).toContain('todo-panel-status-tag');
    expect(text).toContain('>waiting<');
    expect(text).not.toContain('>abandoned<');
  });

  it('tags ONLY the two engine-written statuses', () => {
    // The three the agent writes are self-evident from the row's own styling;
    // a tag on each would be noise on every row of every list.
    const items: TodoItem[] = [
      { content: 'a', active_form: 'doing a', status: 'pending' },
      { content: 'b', active_form: 'doing b', status: 'in_progress' },
      { content: 'c', active_form: 'doing c', status: 'completed' },
    ];
    const text = vnodeToText(todoListPanelBody({ items, onClose: NOOP }));
    expect(text).not.toContain('todo-panel-status-tag');
  });

  // ADR 0085 decision 1: the notes render beside the items, and only when
  // there are any. The mode is off by default, so a list without notes has to
  // be exactly the panel it was before the field existed.
  it('renders the notes above the list when the agent kept some', () => {
    const items: TodoItem[] = [
      { content: 'Run tests', active_form: 'Running tests', status: 'pending' },
    ];
    const text = vnodeToText(
      todoListPanelBody({ items, notes: 'collect.sh needs bash 5', onClose: NOOP }),
    );
    expect(text).toContain('data-role="todo-notes"');
    expect(text).toContain('collect.sh needs bash 5');
    expect(text.indexOf('todo-panel-notes')).toBeLessThan(text.indexOf('todo-panel-list'));
  });

  it('renders no notes region for a list that carries none', () => {
    const items: TodoItem[] = [
      { content: 'Run tests', active_form: 'Running tests', status: 'pending' },
    ];
    for (const notes of [undefined, null, '']) {
      const text = vnodeToText(todoListPanelBody({ items, notes, onClose: NOOP }));
      expect(text).not.toContain('todo-panel-notes');
      expect(text).toContain('todo-panel-list');
    }
  });

  it('renders the notes for a cleared list, with an empty item list', () => {
    const text = vnodeToText(
      todoListPanelBody({ items: [], notes: 'the report is at artifacts/week.md', onClose: NOOP }),
    );
    expect(text).toContain('the report is at artifacts/week.md');
    expect(text).not.toContain('todo-panel-row');
  });
});
