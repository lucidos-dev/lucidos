/**
 * Prompt history in the composer: what counts as history, where Up and Down
 * take the caret's place, and how the draft is stashed and restored.
 *
 * Plan: docs/plans/2026-11-11-prompt-history-recall.md
 */
import { describe, it, expect, beforeEach } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import type { StoredEvent, ThreadState } from '../../../store/thread-events';
import { endPromptHistory, sentPrompts, stepPromptHistory, type HistoryKeypress } from '../promptHistory';

const here: string = dirname(fileURLToPath(import.meta.url));
const promptSource = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');

const THREAD = 't1';

function at(seconds: number): string {
  return new Date(Date.UTC(2026, 10, 11, 9, 0, seconds)).toISOString();
}

function threadWith(...events: Array<Partial<StoredEvent>>): ThreadState {
  const map = new Map<number, StoredEvent>();
  events.forEach((event, i) => map.set(i + 1, { created: at(i), ...event } as StoredEvent));
  return { events: map } as unknown as ThreadState;
}

function sent(text: string, mode: 'human' | 'agent' | 'engine' = 'human'): Partial<StoredEvent> {
  return { type: 'MessageReceived', text, mode } as Partial<StoredEvent>;
}

/** A keypress with the caret at `caret` (default: the start, or the end for Down). */
function press(direction: 'up' | 'down', text: string, caret?: number, extra: Partial<HistoryKeypress> = {}): HistoryKeypress {
  const at = caret ?? (direction === 'up' ? 0 : text.length);
  return { direction, text, selectionStart: at, selectionEnd: at, modified: false, ...extra };
}

beforeEach(() => {
  endPromptHistory(THREAD);
});

describe('sentPrompts', () => {
  it('lists the human prompts oldest first', () => {
    expect(sentPrompts(threadWith(sent('one'), sent('two')))).toEqual(['one', 'two']);
  });

  it('leaves out agent and engine messages', () => {
    const thread = threadWith(sent('mine'), sent('from a parent', 'agent'), sent('keep-alive', 'engine'));
    expect(sentPrompts(thread)).toEqual(['mine']);
  });

  it('leaves out other event types, blank texts and an immediate repeat', () => {
    const thread = threadWith(
      sent('one'),
      sent('one'),
      sent('   '),
      { type: 'ResponseGenerated', text: 'reply' } as Partial<StoredEvent>,
      sent('two'),
      sent('one'),
    );
    expect(sentPrompts(thread)).toEqual(['one', 'two', 'one']);
  });

  it('orders by time, not by arrival', () => {
    const map = new Map<number, StoredEvent>();
    map.set(1, { ...sent('later'), created: at(5) } as StoredEvent);
    map.set(2, { ...sent('earlier'), created: at(1) } as StoredEvent);
    expect(sentPrompts({ events: map } as unknown as ThreadState)).toEqual(['earlier', 'later']);
  });

  it('is empty for a thread that is not loaded', () => {
    expect(sentPrompts(undefined)).toEqual([]);
  });
});

describe('Up from the start of the box', () => {
  const history = ['first', 'second', 'third'];

  it('recalls the newest prompt into an empty box, then older ones', () => {
    expect(stepPromptHistory(THREAD, press('up', ''), history)).toBe('third');
    expect(stepPromptHistory(THREAD, press('up', 'third', 0), history)).toBe('second');
    expect(stepPromptHistory(THREAD, press('up', 'second', 0), history)).toBe('first');
  });

  it('stops at the oldest prompt and leaves the key to the browser', () => {
    stepPromptHistory(THREAD, press('up', ''), ['only']);
    expect(stepPromptHistory(THREAD, press('up', 'only', 0), ['only'])).toBeNull();
  });

  it('does nothing when there is no history', () => {
    expect(stepPromptHistory(THREAD, press('up', ''), [])).toBeNull();
  });

  it('keeps native Up while the caret is mid-text in a typed draft', () => {
    expect(stepPromptHistory(THREAD, press('up', 'draft text', 5), history)).toBeNull();
    expect(stepPromptHistory(THREAD, press('up', 'draft text', 10), history)).toBeNull();
  });

  it('recalls from the start of a typed draft', () => {
    expect(stepPromptHistory(THREAD, press('up', 'draft text', 0), history)).toBe('third');
  });

  it('keeps native Up on a selection', () => {
    const selection = { selectionStart: 0, selectionEnd: 4 };
    expect(stepPromptHistory(THREAD, press('up', 'draft', 0, selection), history)).toBeNull();
  });

  it('keeps Meta, Ctrl, Alt and Shift with Up', () => {
    expect(stepPromptHistory(THREAD, press('up', '', 0, { modified: true }), history)).toBeNull();
  });
});

describe('Up on a recalled entry left as it was', () => {
  const history = ['first', 'a\nb\nc'];

  it('walks a multi-line entry by line, then recalls from its first line', () => {
    stepPromptHistory(THREAD, press('up', ''), history);
    const entry = 'a\nb\nc';
    expect(stepPromptHistory(THREAD, press('up', entry, entry.length), history)).toBeNull();
    expect(stepPromptHistory(THREAD, press('up', entry, 2), history)).toBeNull();
    expect(stepPromptHistory(THREAD, press('up', entry, 1), history)).toBe('first');
  });

  it('keeps native Up once the entry is edited, until the caret is at the start', () => {
    stepPromptHistory(THREAD, press('up', ''), history);
    expect(stepPromptHistory(THREAD, press('up', 'a\nb\nc!', 6), history)).toBeNull();
    expect(stepPromptHistory(THREAD, press('up', 'a\nb\nc!', 0), history)).toBe('first');
  });
});

describe('Down', () => {
  const history = ['first', 'second'];

  it('does nothing when the user is not browsing', () => {
    expect(stepPromptHistory(THREAD, press('down', 'draft'), history)).toBeNull();
  });

  it('walks to newer prompts, then restores the draft typed before the first Up', () => {
    expect(stepPromptHistory(THREAD, press('up', 'my draft', 0), history)).toBe('second');
    expect(stepPromptHistory(THREAD, press('up', 'second', 0), history)).toBe('first');
    expect(stepPromptHistory(THREAD, press('down', 'first'), history)).toBe('second');
    expect(stepPromptHistory(THREAD, press('down', 'second'), history)).toBe('my draft');
  });

  it('ends the walk once the draft is restored', () => {
    stepPromptHistory(THREAD, press('up', 'my draft', 0), history);
    stepPromptHistory(THREAD, press('down', 'second'), history);
    expect(stepPromptHistory(THREAD, press('down', 'my draft'), history)).toBeNull();
  });

  it('keeps native Down while the caret is mid-text in an edited entry', () => {
    stepPromptHistory(THREAD, press('up', ''), history);
    expect(stepPromptHistory(THREAD, press('down', 'second!', 2), history)).toBeNull();
  });

  it('walks a multi-line entry by line before it leaves', () => {
    const lines = ['x', 'a\nb'];
    stepPromptHistory(THREAD, press('up', ''), lines);
    expect(stepPromptHistory(THREAD, press('down', 'a\nb', 1), lines)).toBeNull();
    expect(stepPromptHistory(THREAD, press('down', 'a\nb', 3), lines)).toBe('');
  });

  it('drops an edit to a recalled entry but keeps the stash', () => {
    stepPromptHistory(THREAD, press('up', 'my draft', 0), history);
    stepPromptHistory(THREAD, press('up', 'second', 0), history);
    expect(stepPromptHistory(THREAD, press('down', 'first, edited'), history)).toBe('second');
    expect(stepPromptHistory(THREAD, press('down', 'second'), history)).toBe('my draft');
  });

  it('keeps Meta, Ctrl, Alt and Shift with Down', () => {
    stepPromptHistory(THREAD, press('up', ''), history);
    expect(stepPromptHistory(THREAD, press('down', 'second', undefined, { modified: true }), history)).toBeNull();
  });
});

describe('ending the walk', () => {
  it('starts again at the newest prompt after a send', () => {
    const history = ['first', 'second'];
    stepPromptHistory(THREAD, press('up', ''), history);
    stepPromptHistory(THREAD, press('up', 'second', 0), history);
    endPromptHistory(THREAD);
    expect(stepPromptHistory(THREAD, press('up', ''), history)).toBe('second');
  });

  it('starts again when a new prompt joined the history under the walk', () => {
    stepPromptHistory(THREAD, press('up', ''), ['first', 'second']);
    expect(stepPromptHistory(THREAD, press('up', '', 0), ['first', 'second', 'third'])).toBe('third');
  });

  it('keeps the stash when the history shifts under the walk', () => {
    stepPromptHistory(THREAD, press('up', 'my draft', 0), ['first', 'second']);
    expect(stepPromptHistory(THREAD, press('down', 'second'), ['older', 'first', 'second'])).toBe('my draft');
  });

  it('keeps the stash across a restarted walk', () => {
    stepPromptHistory(THREAD, press('up', 'my draft', 0), ['first', 'second']);
    const shifted = ['first', 'second', 'third'];
    expect(stepPromptHistory(THREAD, press('up', 'second', 0), shifted)).toBe('third');
    expect(stepPromptHistory(THREAD, press('down', 'third'), shifted)).toBe('my draft');
  });

  it('walks each thread on its own', () => {
    const history = ['first', 'second'];
    stepPromptHistory(THREAD, press('up', ''), history);
    expect(stepPromptHistory('t2', press('up', ''), history)).toBe('second');
    endPromptHistory('t2');
  });
});

describe('the composer wiring', () => {
  it('writes a recall through the draft and asks for an end caret', () => {
    const recall = promptSource.slice(promptSource.indexOf('function recallPromptHistory'), promptSource.indexOf('function handleKeyDown'));
    expect(recall).toContain('updateCompose(threadId, { text })');
    expect(recall).toContain('requestPromptCaretToEnd()');
    expect(recall).not.toMatch(/el\.value\s*=/);
  });

  it('tries history after Escape and before Enter', () => {
    const keydown = promptSource.slice(promptSource.indexOf('function handleKeyDown'));
    const order = ["e.key === 'Escape'", 'recallPromptHistory(e)', 'composerEnterSends(e'].map((part) => keydown.indexOf(part));
    expect(order.every((i) => i >= 0)).toBe(true);
    expect([...order].sort((a, b) => a - b)).toEqual(order);
  });

  it('ends the walk when a message is sent, never on a refused send', () => {
    const submit = promptSource.slice(promptSource.indexOf('async function submit()'));
    const firstEnd = submit.indexOf('endPromptHistory(threadId)');
    expect(firstEnd).toBeGreaterThan(0);
    for (const refusal of ["sideQuestion.kind === 'refuse'", "uploadsGate(threadId) === 'failed'"]) {
      expect(submit.indexOf(refusal)).toBeGreaterThan(0);
      expect(submit.indexOf(refusal)).toBeLessThan(firstEnd);
    }
    const sendGate = submit.indexOf('await passSendGate(threadId)');
    expect(submit.indexOf('endPromptHistory(threadId)', sendGate)).toBeGreaterThan(sendGate);
  });
});
