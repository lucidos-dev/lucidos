import { describe, it, expect, beforeAll, afterAll, beforeEach, vi } from 'vitest';


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
  _resetDeadKeystrokeProbeForTesting,
} from '../deadKeystrokeProbe';

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

  it('says nothing when the composer itself emptied the box after the input', () => {
    const el = box('/harden');
    fire('input', { target: el });
    el.value = '';
    vi.advanceTimersByTime(10);
    expect(verdicts()).toEqual([]);
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

  it('caps one episode, then goes quiet until a keystroke lands cleanly', () => {
    for (let i = 0; i < 12; i++) {
      fire('input', { target: box('hello') });
      vi.advanceTimersByTime(10);
    }
    expect(verdicts()).toHaveLength(5);
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
