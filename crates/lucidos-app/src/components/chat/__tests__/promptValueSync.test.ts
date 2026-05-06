import { describe, it, expect } from 'vitest';
import { syncTextareaValue, shouldSkipSyncWhileEditing } from '../promptValueSync';

function makeTextarea(initial: { value: string; selectionStart: number; selectionEnd: number }) {
  const el = {
    value: initial.value,
    selectionStart: initial.selectionStart,
    selectionEnd: initial.selectionEnd,
    setSelectionRange(start: number, end: number) {
      this.selectionStart = start;
      this.selectionEnd = end;
    },
  };
  return el as unknown as HTMLTextAreaElement;
}

describe('syncTextareaValue', () => {
  it('returns false and does not touch the DOM when the value already matches', () => {
    const el = makeTextarea({ value: 'hello world', selectionStart: 5, selectionEnd: 5 });
    const changed = syncTextareaValue(el, 'hello world', true);
    expect(changed).toBe(false);
    expect(el.value).toBe('hello world');
    expect(el.selectionStart).toBe(5);
    expect(el.selectionEnd).toBe(5);
  });

  it('preserves the cursor in the middle when text is replaced on the same thread', () => {
    const el = makeTextarea({ value: 'Hello world', selectionStart: 5, selectionEnd: 5 });
    syncTextareaValue(el, 'Hello there', true);
    expect(el.value).toBe('Hello there');
    expect(el.selectionStart).toBe(5);
    expect(el.selectionEnd).toBe(5);
  });

  it('clamps the cursor to the new text length when the new value is shorter', () => {
    const el = makeTextarea({ value: 'Hello world', selectionStart: 11, selectionEnd: 11 });
    syncTextareaValue(el, 'Hi', true);
    expect(el.value).toBe('Hi');
    expect(el.selectionStart).toBe(2);
    expect(el.selectionEnd).toBe(2);
  });

  it('preserves a non-empty selection range', () => {
    const el = makeTextarea({ value: 'Hello world', selectionStart: 0, selectionEnd: 5 });
    syncTextareaValue(el, 'Hello there', true);
    expect(el.selectionStart).toBe(0);
    expect(el.selectionEnd).toBe(5);
  });

  it('does NOT call setSelectionRange when preserveCursor is false (thread switch)', () => {
    // Real browsers snap to end on `el.value =`; the mock leaves selection
    // untouched, which is what we assert — the helper didn't intervene.
    const el = makeTextarea({ value: 'Hello world', selectionStart: 5, selectionEnd: 5 });
    syncTextareaValue(el, 'Different thread text', false);
    expect(el.value).toBe('Different thread text');
    expect(el.selectionStart).toBe(5);
    expect(el.selectionEnd).toBe(5);
  });

  it('clamps an out-of-range selection start to text.length', () => {
    const el = makeTextarea({ value: 'Hello world', selectionStart: 9, selectionEnd: 11 });
    syncTextareaValue(el, 'Hi', true);
    expect(el.selectionStart).toBe(2);
    expect(el.selectionEnd).toBe(2);
  });

  // Pinning regression for the user-visible "cursor jumps forward 1–2 chars
  // before/during my keystroke" bug. Mechanism: a stale composeText (e.g.
  // an SSE race past the upstream guards) overwrites el.value while the
  // user just typed an extra char. Cursor preservation pins the numeric
  // index, which in the now-shorter text sits 1 char later visually — the
  // next keystroke lands there. PromptInput's useEffect avoids the call
  // via shouldSkipSyncWhileEditing whenever the user has local content
  // here, so this shape is unreachable in practice. Test pins the
  // underlying mechanism.
  it('REGRESSION: stale overwrite + cursor preservation visually shifts caret in shorter text', () => {
    // User had "Hello world" (length 11) and just typed "X" between "Hello"
    // and " world", giving local "HelloX world" (length 12) with cursor at 6.
    // SSE arrives with the pre-X text "Hello world".
    const el = makeTextarea({ value: 'HelloX world', selectionStart: 6, selectionEnd: 6 });
    syncTextareaValue(el, 'Hello world', true);
    // X is gone; cursor is at index 6, but in "Hello world" position 6 is
    // between space and 'w' — visually 1 character LATER than where the
    // user typed X (between "Hello" and the space). Next keystroke "Y"
    // would land at index 6 → "Hello Yworld" instead of the intended
    // "HelloXY world".
    expect(el.value).toBe('Hello world');
    expect(el.selectionStart).toBe(6);
    expect(el.value[5]).toBe(' ');
    expect(el.value[6]).toBe('w');
  });
});

describe('shouldSkipSyncWhileEditing', () => {
  function makeEl(value: string): HTMLTextAreaElement {
    return { value } as HTMLTextAreaElement;
  }

  it('skips sync when the user is focused here on the same thread with local content', () => {
    expect(shouldSkipSyncWhileEditing(makeEl('hello'), true, true)).toBe(true);
  });

  it('does NOT skip when the textarea is empty — empty el.value cannot host a destroyed keystroke, so the persisted draft must reach the input on initial autofocus after reload', () => {
    expect(shouldSkipSyncWhileEditing(makeEl(''), true, true)).toBe(false);
  });

  it('does NOT skip on thread switch (sameThread=false) — the new thread\'s text is the right value', () => {
    expect(shouldSkipSyncWhileEditing(makeEl('stale-from-prev-thread'), false, true)).toBe(false);
  });

  it('does NOT skip when focus is elsewhere — no in-flight keystroke to protect', () => {
    expect(shouldSkipSyncWhileEditing(makeEl('hello'), true, false)).toBe(false);
  });
});
