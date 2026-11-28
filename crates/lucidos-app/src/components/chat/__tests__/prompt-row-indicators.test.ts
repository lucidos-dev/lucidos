import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { todoIndicatorSummary } from '../todoIndicator';
import { waitingIndicatorSummary } from '../WaitingPanel';
import type { TodoItem } from '../../../store/thread-events';

const here: string = dirname(fileURLToPath(import.meta.url));
const prompt = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');

const todo = (status: TodoItem['status'], content = 'a step'): TodoItem =>
  ({ content, status, active_form: `doing ${content}` } as TodoItem);

describe('a folded todo indicator says what the glyph could not', () => {
  it('reports nothing when the agent has written no list', () => {
    expect(todoIndicatorSummary(null)).toBeNull();
    expect(todoIndicatorSummary([], null)).toBeNull();
  });

  it('counts the list in words', () => {
    const summary = todoIndicatorSummary([todo('completed'), todo('pending')]);
    expect(summary?.menuLabel).toBe('Todo list: 1 of 2 done');
  });

  it('names the item in flight', () => {
    const summary = todoIndicatorSummary([todo('in_progress', 'the migration'), todo('pending')]);
    expect(summary?.menuLabel).toContain('doing the migration');
  });

  it('keeps notes reachable after the list empties', () => {
    expect(todoIndicatorSummary([], 'a pointer worth keeping')?.menuLabel)
      .toBe('Todo list: notes kept');
  });

  it('says the same thing to a screen reader as to the menu', () => {
    const summary = todoIndicatorSummary([todo('completed')]);
    expect(summary?.ariaLabel).toBe(`${summary?.menuLabel}. Click to expand.`);
  });
});

describe('a folded waiting indicator says what it is waiting for', () => {
  const noChildren = { threads: [], unresolved: 0 };

  it('reports nothing when the thread is parked on nothing', () => {
    expect(waitingIndicatorSummary([], noChildren)).toBeNull();
  });

  it('names a lone subscription by its reason', () => {
    const waits = [{ reason: 'waiting for the release build' }] as never[];
    expect(waitingIndicatorSummary(waits, noChildren)?.menuLabel)
      .toBe('Waiting for the release build');
  });

  it('counts each kind once there is more than one reason', () => {
    const waits = [{ reason: 'one' }, { reason: 'two' }] as never[];
    expect(waitingIndicatorSummary(waits, { threads: [], unresolved: 1 })?.menuLabel)
      .toBe('Waiting for 2 events, 1 sub-thread');
  });

  it('says the same thing to a screen reader as to the menu', () => {
    const summary = waitingIndicatorSummary([{ reason: 'one' }] as never[], noChildren);
    expect(summary?.ariaLabel).toBe(`${summary?.menuLabel}. Click to expand.`);
  });
});

describe('the composer wires both indicators as foldable members', () => {
  it('gates the todo action on the backend and the waiting action on nothing', () => {
    expect(prompt).toMatch(
      /const todoAction = promptCodingAgent === null \? todoIndicatorAction\(\) : null;/,
    );
    expect(prompt).toMatch(/const waitingAction = waitingIndicatorAction\(\);/);
  });

  it('puts todo before waiting, and leaves the WIP preview out of them', () => {
    const todoAt = prompt.indexOf('if (todoAction) readouts.push(todoAction);');
    const waitingAt = prompt.indexOf('if (waitingAction) readouts.push(waitingAction);');
    expect(todoAt).toBeGreaterThan(-1);
    expect(waitingAt).toBeGreaterThan(todoAt);
    expect(prompt).toContain('wipPreview: wipPreviewAction(focusedThread),');
  });

  it('mounts both panels outside the fold cluster', () => {
    expect(prompt).toMatch(/<TodoPanelSlot \/>/);
    expect(prompt).toMatch(/<WaitingPanelHost \/>/);
    expect(prompt.indexOf('<TodoPanelSlot />')).toBeGreaterThan(prompt.indexOf('<OverflowMenu'));
  });

  it('retires the stranded popovers on a fold step and every one on a ⋯ open', () => {
    expect(prompt).toMatch(
      /function closeFoldedPanels\(\): void \{\s*attachMenuOpen\.value = false;\s*closeTodoPanel\(\);\s*closeWaitingPanel\(\);\s*\}/,
    );
    expect(prompt).toMatch(/function closeRowPanels\(\): void \{\s*closeFoldedPanels\(\);\s*closeControlMenus\(\);\s*\}/);
    expect(prompt).toMatch(/useEffect\(closeFoldedPanels, \[collapsedActions\]\);/);
    expect(prompt).toMatch(/onOpen=\{closeRowPanels\}/);
  });
});
