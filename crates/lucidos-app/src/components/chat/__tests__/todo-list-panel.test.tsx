import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { todoListIndicatorBody, todoListPanelBody } from '../TodoListPanel';
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

const NOOP = () => {};

// ──────────────────────────────────────────────────────────────────────────
// Indicator — hidden when no items, otherwise shows a check / in-progress
// SVG icon (no count). SVGs are pixel-identical to the adjacent ImageIcon
// because both inherit `.icon-btn.header-icon svg` sizing (--icon-size-lg).
// Counts go into the tooltip + aria-label instead of the glyph. Tap target
// opens the panel.
// ──────────────────────────────────────────────────────────────────────────

describe('todoListIndicatorBody', () => {
  it('renders nothing when items is null (never written)', () => {
    expect(todoListIndicatorBody({ items: null, onClick: NOOP })).toBeNull();
  });

  it('renders nothing when items is empty (explicitly cleared)', () => {
    expect(todoListIndicatorBody({ items: [], onClick: NOOP })).toBeNull();
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

  it('renders the abandoned state when no item is in progress but some were abandoned', () => {
    const items: TodoItem[] = [
      { content: 'a', active_form: 'doing a', status: 'completed' },
      { content: 'b', active_form: 'doing b', status: 'abandoned' },
      { content: 'c', active_form: 'doing c', status: 'abandoned' },
    ];
    const text = vnodeToText(todoListIndicatorBody({ items, onClick: NOOP }));
    expect(text).toContain('data-state="abandoned"');
    expect(text).not.toContain('data-state="idle"');
    expect(text).not.toContain('data-state="in-progress"');
    expect(text).toContain('1 of 3 done, 2 abandoned');
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
    expect(text).toContain('data-role="todo-panel"');
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
    expect(text).toContain('data-role="todo-panel"');
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
    expect(text).toContain('todo-panel-abandoned-tag');
    expect(text).toContain('>abandoned<');
  });
});
