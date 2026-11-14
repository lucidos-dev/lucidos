import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { syncTextareaValue, requestPromptCaretToEnd, promptCaretToEndSeq } from '../promptValueSync';

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
  // next keystroke lands there. The compose store never replaces a draft
  // with an edit pending, so this shape is unreachable in practice. Test
  // pins the underlying mechanism.
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

/** The box shows the draft, and nothing else, after every render (C8 in
 *  `docs/plans/2026-10-12-compose-draft-follows-a-server-revision.md`). That
 *  is safe because the store never replaces a draft with an edit pending,
 *  which `store/__tests__/compose-follows-server-revision.test.ts` pins. */
describe('the sync effect shows the draft unconditionally', () => {
  const source = readFileSync(
    resolve(dirname(fileURLToPath(import.meta.url)), '../PromptInput.tsx'),
    'utf-8',
  );

  it('writes the draft into the box with no skip, adopt or override branch', () => {
    expect(source).toContain('syncTextareaValue(el, composeText, sameThread && !caretToEnd)');
    expect(source).not.toMatch(/shouldSkipSyncWhileEditing|resolveEmptyDraftSync|typedSinceComposerWrote|adoptComposerText/);
  });

  it('never reads the box where it should read the draft', () => {
    const code = source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '');
    const submit = code.match(/async function submit\(\)[\s\S]*?\n {2}\}/)![0];
    const multi = code.match(/async function submitMultiAnswer\(\)[\s\S]*?\n {2}\}/)![0];
    expect(submit).not.toMatch(/el\.value|\.value\.length/);
    expect(multi).not.toMatch(/el\.value|box\.value/);
  });
});

describe('requestPromptCaretToEnd', () => {
  it('bumps the ticket the sync effect compares against', () => {
    const before = promptCaretToEndSeq.value;
    requestPromptCaretToEnd();
    expect(promptCaretToEndSeq.value).toBe(before + 1);
  });
});
