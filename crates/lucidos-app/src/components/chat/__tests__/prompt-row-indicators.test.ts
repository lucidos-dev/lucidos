/** The two STATUS readouts fold, and a folded one still reports.
 *
 *  Pinning them was the first pass, on the reasoning that a readout inside a
 *  closed menu reports nothing. Measured against the reported row, it removed
 *  one glyph of seven and left the row at its width limit. So it did not answer
 *  the report. They fold now, and each says its state in words on its menu row.
 *  Colour and a pulse do not survive the fold, and words do.
 *
 *  A source scan for the wiring and a unit test for the words. Rendering the
 *  composer pulls in the whole chat surface, which every test beside this one
 *  avoids for the same reason.
 *
 *  Plan: `docs/plans/2026-09-19-the-composer-row-is-one-row.md`. */
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

  /** The in-progress item is what the coloured glyph was saying. */
  it('names the item in flight', () => {
    const summary = todoIndicatorSummary([todo('in_progress', 'the migration'), todo('pending')]);
    expect(summary?.menuLabel).toContain('doing the migration');
  });

  it('keeps notes reachable after the list empties', () => {
    expect(todoIndicatorSummary([], 'a pointer worth keeping')?.menuLabel)
      .toBe('Todo list: notes kept');
  });

  /** The row button's accessible name and the menu row's words come from one
   *  summary, so neither can drift from the other. */
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

  /** A lone subscription reads as its own reason, and the label supplies the
   *  verb the reason does not. */
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
  /** `TodoListWritten` is the Lucidos Agent's own event, so a coding-agent
   *  thread has no list. The WAIT is not backend-specific: both agents arm
   *  them, and a coding-agent thread sat in Waiting with nowhere to read what
   *  it watched before the indicator reached it. */
  it('gates the todo action on the backend and the waiting action on nothing', () => {
    expect(prompt).toMatch(
      /const todoAction = promptCodingAgent === null \? todoIndicatorAction\(\) : null;/,
    );
    expect(prompt).toMatch(/const waitingAction = waitingIndicatorAction\(\);/);
  });

  /** They head the list, so they are the first things the ⋯ menu takes. An
   *  action the thumb reaches for outranks a readout. */
  it('puts them at the head of the foldable list', () => {
    const todoAt = prompt.indexOf('if (todoAction) foldActions.push(todoAction);');
    const waitingAt = prompt.indexOf('if (waitingAction) foldActions.push(waitingAction);');
    const wipAt = prompt.indexOf('if (wipAction) foldActions.push(wipAction);');
    expect(todoAt).toBeGreaterThan(-1);
    expect(waitingAt).toBeGreaterThan(todoAt);
    expect(wipAt).toBeGreaterThan(waitingAt);
  });

  /** A panel cannot live inside the control that folds away. Both are mounted
   *  by the composer and portal out of it. */
  it('mounts both panels outside the fold cluster', () => {
    expect(prompt).toMatch(/<TodoPanelSlot \/>/);
    expect(prompt).toMatch(/<WaitingPanelHost \/>/);
    expect(prompt.indexOf('<TodoPanelSlot />')).toBeGreaterThan(prompt.indexOf('<OverflowMenu'));
  });

  /** Two ways a panel's anchor stops being a box worth pointing at. A fold step
   *  can unmount it. And the ⋯ trigger is exempt from the dismiss of the panel
   *  it anchors, so re-pressing it would stack one over the other. */
  it('retires every composer popover on a fold step and on a ⋯ open', () => {
    expect(prompt).toMatch(
      /function closeFoldedPanels\(\): void \{\s*attachMenuOpen\.value = false;\s*closeTodoPanel\(\);\s*closeWaitingPanel\(\);\s*\}/,
    );
    expect(prompt).toMatch(/useEffect\(closeFoldedPanels, \[collapsedActions\]\);/);
    expect(prompt).toMatch(/onOpen=\{closeFoldedPanels\}/);
  });
});
