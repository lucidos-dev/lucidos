import { describe, it, expect, beforeAll, afterAll, beforeEach, vi } from 'vitest';

// The typing half of the dead-composer report, driven through the document
// stub. The ninth episode was "I typed and the text did not appear", and the
// press probe was silent and correct: no button had been pressed. Nothing
// watched the textarea, so the episode left no line at all.
//
// docs/plans/2026-08-29-the-composer-never-erases-what-you-typed.md

const postClientLog = vi.hoisted(() => vi.fn());
const isMobile = vi.hoisted(() => vi.fn(() => true));
const draftText = vi.hoisted(() => ({ value: '' }));
const focusedId = vi.hoisted(() => ({ value: 't-1' as string | null }));

vi.mock('../../../utils/clientLog', () => ({ postClientLog }));
vi.mock('../../../utils/viewport', () => ({ isMobile }));
vi.mock('../../../store/composeDrafts', () => ({
  getDraft: () => ({ text: draftText.value, image_hashes: [], mode: null }),
}));
vi.mock('../../../store/store', () => ({ focusedThreadId: focusedId }));

import {
  installDeadKeystrokeProbe,
  reportDraftClobbered,
  _resetDeadKeystrokeProbeForTesting,
} from '../deadKeystrokeProbe';

/** The smallest element the probe reads: a role marker and a value. */
function box(value: string) {
  return { dataset: { role: 'prompt-input' }, value };
}

function fire(type: string, event: Record<string, unknown>) {
  const spies = { preventDefault: vi.fn(), stopPropagation: vi.fn() };
  (globalThis.document as unknown as { dispatchEvent(e: unknown): void })
    .dispatchEvent({ type, ...spies, ...event });
  return spies;
}

function setActive(el: unknown) {
  (globalThis.document as unknown as Record<string, unknown>).activeElement = el;
}

/** Every `composer-typing` line written so far, newest last. */
function lines(): Array<Record<string, unknown>> {
  return postClientLog.mock.calls
    .filter((c) => c[0] === 'composer-typing')
    .map((c) => c[2] as Record<string, unknown>);
}

function verdicts(): string[] {
  return lines().map((l) => l.verdict as string);
}

beforeAll(() => {
  vi.useFakeTimers();
  _resetDeadKeystrokeProbeForTesting();
  installDeadKeystrokeProbe();
});

afterAll(() => {
  vi.useRealTimers();
  setActive(null);
});

beforeEach(() => {
  postClientLog.mockClear();
  isMobile.mockReturnValue(true);
  draftText.value = '';
  focusedId.value = 't-1';
  setActive(null);
  // Drain any deadline left armed by the previous case, then forget its lines.
  vi.advanceTimersByTime(5000);
  postClientLog.mockClear();
  _resetDeadKeystrokeProbeForTesting();
});

describe('a keystroke that reaches the box and not the store', () => {
  it('writes one line naming the two lengths', () => {
    fire('input', { target: box('hello') });
    vi.advanceTimersByTime(10);
    expect(verdicts()).toEqual(['keystroke-lost']);
    expect(lines()[0]).toMatchObject({ charCount: 5, draftCharCount: 0, hasFocusedThread: true });
  });

  it('says nothing when the draft took the keystroke, which is every ordinary one', () => {
    draftText.value = 'hello';
    fire('input', { target: box('hello') });
    vi.advanceTimersByTime(10);
    expect(verdicts()).toEqual([]);
  });

  // On a composing thread the composer auto-discards a draft the user emptied,
  // so a whitespace-only box legitimately has no draft behind it. Comparing it
  // would report the app's own correct behaviour as a fault.
  it('says nothing for a whitespace-only box', () => {
    fire('input', { target: box('   ') });
    vi.advanceTimersByTime(10);
    expect(verdicts()).toEqual([]);
  });

  it('ignores input from anything that is not the prompt textarea', () => {
    fire('input', { target: { dataset: { role: 'search-box' }, value: 'hello' } });
    vi.advanceTimersByTime(10);
    expect(verdicts()).toEqual([]);
  });

  // The composer rewrites the box inside its own `onInput`: a leading "/" opens
  // the slash menu and empties the field. Comparing the value captured at input
  // time would call that a lost keystroke on every slash command.
  it('says nothing when the composer itself emptied the box after the input', () => {
    const el = box('/harden');
    fire('input', { target: el });
    el.value = '';
    vi.advanceTimersByTime(10);
    expect(verdicts()).toEqual([]);
  });
});

describe('a clear that ran under the user’s fingers', () => {
  // The composer repairs it, keeping the characters and handing them to the
  // draft (`resolveEmptyDraftSync`). The line is the only trace the repair
  // leaves, and the fault is the clear, not the repair.
  it('records the repair on the same channel, with the thread’s status', () => {
    reportDraftClobbered({ charCount: 24, threadStatus: 'running' });
    expect(verdicts()).toEqual(['draft-clobbered']);
    expect(lines()[0]).toMatchObject({ charCount: 24, threadStatus: 'running' });
  });

  it('carries the viewport block, so it reads beside a press episode', () => {
    reportDraftClobbered({ charCount: 3, threadStatus: 'idle' });
    expect(lines()[0].viewport).toMatchObject({ keyboardActive: false });
  });

  // A clear can only be repaired on the device the user is typing on, and the
  // composer calls this directly rather than through a listener.
  it('is not gated on mobile, unlike the listeners', () => {
    isMobile.mockReturnValue(false);
    reportDraftClobbered({ charCount: 1, threadStatus: 'idle' });
    expect(verdicts()).toEqual(['draft-clobbered']);
  });
});

describe('an edit the browser announced and never delivered', () => {
  it('writes a line when no input follows and the box still holds focus', () => {
    const el = box('');
    setActive(el);
    fire('beforeinput', { target: el, inputType: 'insertText' });
    vi.advanceTimersByTime(500);
    expect(verdicts()).toEqual(['input-never-arrived']);
    expect(lines()[0]).toMatchObject({ inputType: 'insertText' });
  });

  it('says nothing when the input arrives behind it', () => {
    const el = box('a');
    draftText.value = 'a';
    setActive(el);
    fire('beforeinput', { target: el, inputType: 'insertText' });
    fire('input', { target: el });
    vi.advanceTimersByTime(500);
    expect(verdicts()).toEqual([]);
  });

  // A blur, a thread switch or a send between the two events legitimately drops
  // the edit, and none of those is the touch pipeline stopping.
  it('says nothing when the box lost focus before the deadline', () => {
    const el = box('');
    setActive(el);
    fire('beforeinput', { target: el, inputType: 'insertText' });
    setActive(null);
    vi.advanceTimersByTime(500);
    expect(verdicts()).toEqual([]);
  });

  it('is mobile-only, the report being an iOS PWA one', () => {
    isMobile.mockReturnValue(false);
    const el = box('');
    setActive(el);
    fire('beforeinput', { target: el, inputType: 'insertText' });
    vi.advanceTimersByTime(500);
    expect(verdicts()).toEqual([]);
  });
});

describe('the probe stays out of the way', () => {
  it('consumes no event: a diagnostic that eats a keystroke becomes the bug', () => {
    const el = box('hello');
    setActive(el);
    const announced = fire('beforeinput', { target: el, inputType: 'insertText' });
    const typed = fire('input', { target: el });
    vi.advanceTimersByTime(500);
    for (const spies of [announced, typed]) {
      expect(spies.preventDefault).not.toHaveBeenCalled();
      expect(spies.stopPropagation).not.toHaveBeenCalled();
    }
  });

  // A wedge persists, and the user's answer to a dead box is to keep typing.
  // Ungated, that writes a line per character for as long as the state lasts.
  it('caps one episode, then goes quiet until a keystroke lands cleanly', () => {
    for (let i = 0; i < 12; i++) {
      fire('input', { target: box('hello') });
      vi.advanceTimersByTime(10);
    }
    expect(verdicts()).toHaveLength(5);
    // A clean keystroke ends the episode, so a state that returns reports again.
    draftText.value = 'hello';
    fire('input', { target: box('hello') });
    vi.advanceTimersByTime(10);
    draftText.value = '';
    fire('input', { target: box('hello') });
    vi.advanceTimersByTime(10);
    expect(verdicts()).toHaveLength(6);
  });

  it('carries no typed text into the log, only lengths and flags', () => {
    const typed = 'my private follow-up';
    draftText.value = '';
    fire('input', { target: box(typed) });
    vi.advanceTimersByTime(10);
    expect(JSON.stringify(lines())).not.toContain(typed);
    expect(lines()[0]).toMatchObject({ charCount: typed.length });
  });
});
