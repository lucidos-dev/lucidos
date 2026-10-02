// @vitest-environment jsdom
/**
 * **Each Changes panel section draws its own bulk row, one line of buttons.**
 *
 * Ready comes first: Discard All at the left edge, Apply All at the right.
 * Not finished comes second, with "Apply all on settle" alone at the right.
 * A line never wraps, and its buttons share it equally at one height.
 *
 * Rendered for the structure, scanned for the geometry: jsdom lays nothing
 * out, so the CSS scan is what pins the two ends and the no-wrap.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

vi.mock('../../../store/actions/threads', () => ({
  focusThreadOrBootstrap: vi.fn(),
  focusThread: vi.fn(),
}));
vi.mock('../../../store/actions/repositories', () => ({
  viewChangeDiff: vi.fn(),
  viewThreadCcDiff: vi.fn(),
}));

import { ChangesView } from '../ChangesView';
import {
  changes,
  appliedChanges,
  setAsideChanges,
  applyingChangeIds,
  applyingNowThreadIds,
  applyAllInProgress,
  standingApplyThreadIds,
  armingStandingApplyThreadIds,
} from '../../../store/store';
import type { Change } from '../../../api/client';
import { cssRules, rulesTargeting, styleSheetPaths } from '../../../styles/__tests__/css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesRoot: string = resolve(here, '../../../styles');
const allCss: string = styleSheetPaths(stylesRoot).map((p) => readFileSync(p, 'utf-8')).join('\n');
const allRules = cssRules(allCss);

function makeChange(over: Partial<Change> = {}): Change {
  return {
    id: 'change-1',
    request_id: '00000000-0000-0000-0000-000000000000',
    thread_id: 'thread-1',
    thread_title: 'Thread',
    branch_name: 'b',
    repo_root: '/r',
    description: 'desc',
    file_count: 1,
    files: ['a.rs'],
    requires_restart: false,
    hardened: true,
    status: 'pending',
    created_at: '2026-01-01T00:00:00Z',
    resolved_at: null,
    pre_merge_sha: null,
    post_merge_sha: null,
    commits: [],
    summary: null,
    incomplete: false,
    ...over,
  };
}

let host: HTMLDivElement;

beforeEach(() => {
  // Two changes ready now, one still settling: every bulk button draws.
  changes.value = {
    status: 'loaded',
    data: [
      makeChange({ id: 'ready', thread_id: 'thread-ready' }),
      makeChange({ id: 'settling', thread_id: 'thread-settling', thread_unsettled: true, thread_settling: true }),
      makeChange({ id: 'ready-2', thread_id: 'thread-ready-2' }),
    ],
  };
  appliedChanges.value = { status: 'loaded', data: [] };
  setAsideChanges.value = { status: 'loaded', data: [] };
  applyingChangeIds.value = new Set();
  applyingNowThreadIds.value = new Map();
  applyAllInProgress.value = false;
  standingApplyThreadIds.value = new Set();
  armingStandingApplyThreadIds.value = new Set();
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

function bulkRows(): HTMLElement[] {
  return [...host.querySelectorAll<HTMLElement>('.changes-bulk-actions')];
}

function buttonLine(row: HTMLElement): HTMLElement {
  const line = row.querySelector<HTMLElement>(':scope > .changes-bulk-buttons');
  if (!line) throw new Error('the bulk row has no button line');
  return line;
}

function labels(el: Element): string[] {
  return [...el.children].map((c) => c.textContent?.trim() ?? '');
}

function rule(selector: string) {
  const found = allRules.find((r) => r.selector === selector && r.atRules === '');
  expect(found, `no top-level rule for ${selector}`).toBeDefined();
  return found!;
}

describe('the two sections', () => {
  it('draws Ready first and Not finished second, each with its own bulk row', () => {
    render(<ChangesView />, host);
    const titles = [...host.querySelectorAll('.list-section-title .section-label')].map((t) => t.textContent);
    expect(titles).toEqual(['Ready', 'Not finished']);
    expect(bulkRows().map((row) => labels(buttonLine(row)))).toEqual([
      ['Discard All', 'Apply All'],
      ['Apply all on settle'],
    ]);
  });

  it('lists each change under its own section', () => {
    render(<ChangesView />, host);
    const [ready, notFinished] = [...host.querySelectorAll('.list-section-title')];
    const between = (a: Element, b: Element | null) => {
      const out: string[] = [];
      for (let el = a.nextElementSibling; el && el !== b; el = el.nextElementSibling) {
        out.push(...[...el.querySelectorAll('.list-row-label')].map((l) => l.textContent ?? ''));
      }
      return out;
    };
    expect(between(ready, notFinished)).toHaveLength(2);
    expect(between(notFinished, null)).toHaveLength(1);
  });

  // Discard never reaches a thread still working.
  it('never draws Discard All in Not finished', () => {
    render(<ChangesView />, host);
    expect(labels(buttonLine(bulkRows()[1]))).not.toContain('Discard All');
  });

  it('draws the armed face alone when nothing else can act', () => {
    changes.value = {
      status: 'loaded',
      data: [makeChange({ id: 'settling', thread_unsettled: true, thread_settling: true })],
    };
    standingApplyThreadIds.value = new Set(['thread-1']);
    render(<ChangesView />, host);
    expect(bulkRows().map((row) => labels(buttonLine(row)))).toEqual([['✓ Applying all on settle']]);
  });
});

// A set-aside row wears the same split button as a pending one: Bring back
// is the face, and Discard waits behind the caret.
describe('the set-aside row', () => {
  function setAsideRowButtons(): string[] {
    const rows = [...host.querySelectorAll('.change-row')];
    const row = rows.find((r) => r.textContent?.includes('Bring back'));
    if (!row) throw new Error('no set-aside row drawn');
    return [...row.querySelectorAll('.list-row-actions button')].map(
      (b) => b.getAttribute('aria-label') ?? b.textContent?.trim() ?? '',
    );
  }

  beforeEach(() => {
    changes.value = { status: 'loaded', data: [] };
    setAsideChanges.value = {
      status: 'loaded',
      data: [makeChange({ id: 'aside', thread_id: null, status: 'set_aside' as Change['status'] })],
    };
  });

  it('draws Bring back as a split button with Discard behind the caret', () => {
    render(<ChangesView />, host);
    expect(setAsideRowButtons()).toEqual(['Diff', 'Bring back', 'More set-aside actions']);
    expect(host.querySelector('.split-button-primary')?.className).not.toContain('action-btn-confirm');
  });
});

// The control after Diff keeps one width on every row. So Diff lines up down
// the list, beside a plain button or a split button alike.
describe('the row action column', () => {
  it('marks the control after Diff on every row as the fixed-width slot', () => {
    setAsideChanges.value = {
      status: 'loaded',
      data: [makeChange({ id: 'aside', thread_id: null, status: 'set_aside' as Change['status'] })],
    };
    render(<ChangesView />, host);
    const rows = [...host.querySelectorAll('.change-row .list-row-actions')];
    expect(rows).toHaveLength(4);
    for (const actions of rows) {
      const [diff, primary] = [...actions.children];
      expect(diff.textContent).toBe('Diff');
      expect(primary.classList.contains('change-row-primary')).toBe(true);
    }
  });

  // The loading row wears the same slot, so Diff does not jump when rows land.
  it('draws the skeleton row with the same fixed-width slot', () => {
    const src: string = readFileSync(resolve(here, '../ChangesView.tsx'), 'utf-8');
    expect(src).toContain('<div class="change-row-primary"><SkBlock w="100%"');
  });

  it('gives that slot one minimum width', () => {
    expect(rule('.change-row-primary').props.get('min-width')).toBe('8rem');
    expect(rule('.change-row-primary .split-button-primary').props.get('flex')).toBe('1');
  });

  // The caret's icon is taller than a label line. With vertical padding, a
  // split-button row would stand taller than a plain-button row.
  it('lets the split button caret add no height of its own', () => {
    expect(rule('.split-button-caret').props.get('padding')).toBe('0 0.4375rem');
  });
});

describe('the bulk row geometry', () => {
  it('keeps the row on the change rows’ gutter, its hairline a row’s distance below', () => {
    expect(rule('.changes-bulk-actions').props.get('padding')).toBe('0 var(--space-lg) var(--space-md)');
  });

  it('packs the button line to the right', () => {
    expect(rule('.changes-bulk-buttons').props.get('justify-content')).toBe('flex-end');
  });

  // Packed right, so Apply All stays right when Discard All is hidden. The
  // auto margin is what sends Discard All to the left edge when it draws.
  it('sends Discard All to the left edge', () => {
    expect(rule('.changes-bulk-buttons > .action-btn-danger').props.get('margin-right')).toBe('auto');
  });

  // Each takes an equal share and wraps inside it on a narrow phone, and the
  // line stretches them all to the tallest one.
  it('gives every button an equal share, one height, and a wrapping label', () => {
    const button = rule('.changes-bulk-buttons > .action-btn').props;
    expect(button.get('flex')).toBe('1 1 0');
    expect(button.get('white-space')).toBe('normal');
    expect(rule('.changes-bulk-buttons').props.get('align-items')).toBe('stretch');
  });

  // Uncapped, a wide desktop pane would stretch each button across half of it.
  it('caps each share so a wide pane keeps the buttons button-sized', () => {
    expect(rule('.changes-bulk-buttons > .action-btn').props.get('max-width')).toBe('10rem');
  });

  it('never wraps the button line, in any sheet or media query', () => {
    const wrapping = rulesTargeting(allCss, 'changes-bulk-buttons')
      .filter((r) => /\bwrap/.test(r.props.get('flex-wrap') ?? '') || /\bwrap/.test(r.props.get('flex-flow') ?? ''));
    expect(wrapping.map((r) => r.selector)).toEqual([]);
  });
});
