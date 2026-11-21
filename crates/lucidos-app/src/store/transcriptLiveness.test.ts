import { describe, it, expect } from 'vitest';
import { signal } from '@preact/signals';
import { watchTranscriptLiveness } from './transcriptLiveness';
import type { ThreadState } from './thread-events';

/** The follow's liveness comes from ONE source, the thread projection of the
 *  thread on screen. A reader's scroll turns the follow off only while it
 *  reads live, and parks the follow otherwise (ADR 0064). */
describe('watchTranscriptLiveness', () => {
  function setup(focusedId: string | null) {
    const quiet = new Set<string>();
    const threads = signal(new Map<string, ThreadState>([
      ['a', { meta: { id: 'a' } } as unknown as ThreadState],
      ['b', { meta: { id: 'b' } } as unknown as ThreadState],
    ]));
    const focused = signal<string | null>(focusedId);
    const pushed: boolean[] = [];
    const dispose = watchTranscriptLiveness({
      focused,
      threads,
      isLive: (t) => !quiet.has(t.meta.id),
      setLive: (live) => { pushed.push(live); },
    });
    /** Mark a thread quiet or streaming, and publish the projection change. */
    const setIdle = (id: string, isQuiet: boolean) => {
      if (isQuiet) quiet.add(id); else quiet.delete(id);
      threads.value = new Map(threads.value);
    };
    return { focused, pushed, setIdle, dispose };
  }

  it('pushes live for a streaming thread on screen, and not live once it waits', () => {
    const { pushed, setIdle, dispose } = setup('a');
    setIdle('a', true);
    expect(pushed).toEqual([true, false]);
    dispose();
  });

  it('follows the focus to the thread now on screen', () => {
    const { focused, pushed, setIdle, dispose } = setup('a');
    setIdle('b', true);
    focused.value = 'b';
    expect(pushed[pushed.length - 1]).toBe(false);
    dispose();
  });

  it('does not re-run on a signal that setLive itself reads', () => {
    const other = signal(0);
    const focused = signal<string | null>(null);
    const threads = signal(new Map<string, ThreadState>());
    let runs = 0;
    const dispose = watchTranscriptLiveness({
      focused,
      threads,
      isLive: () => false,
      setLive: () => { runs++; void other.value; },
    });
    other.value = 1;
    expect(runs).toBe(1);
    dispose();
  });

  it('reads no thread, and an unknown one, as not live', () => {
    const none = setup(null);
    expect(none.pushed).toEqual([false]);
    none.dispose();
    const unknown = setup('missing');
    expect(unknown.pushed).toEqual([false]);
    unknown.dispose();
  });
});
