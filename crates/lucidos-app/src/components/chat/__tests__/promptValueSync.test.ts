import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { syncTextareaValue, requestPromptCaret, requestPromptCaretToEnd, promptCaretRequest, placeTextareaCaret } from '../promptValueSync';

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

  it('REGRESSION: stale overwrite + cursor preservation visually shifts caret in shorter text', () => {
    const el = makeTextarea({ value: 'HelloX world', selectionStart: 6, selectionEnd: 6 });
    syncTextareaValue(el, 'Hello world', true);
    expect(el.value).toBe('Hello world');
    expect(el.selectionStart).toBe(6);
    expect(el.value[5]).toBe(' ');
    expect(el.value[6]).toBe('w');
  });
});

describe('the sync effect shows the draft unconditionally', () => {
  const source = readFileSync(
    resolve(dirname(fileURLToPath(import.meta.url)), '../PromptInput.tsx'),
    'utf-8',
  );

  it('writes the draft into the box with no skip, adopt or override branch', () => {
    expect(source).toContain('syncTextareaValue(el, composeText, sameThread && !caretRequested)');
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

describe('requestPromptCaret', () => {
  it('bumps the ticket the sync effect compares against, end by default', () => {
    const before = promptCaretRequest.value.seq;
    requestPromptCaretToEnd();
    expect(promptCaretRequest.value).toEqual({ seq: before + 1, side: 'end' });
  });

  it('carries the side asked for', () => {
    const before = promptCaretRequest.value.seq;
    requestPromptCaret('start');
    expect(promptCaretRequest.value).toEqual({ seq: before + 1, side: 'start' });
  });
});

describe('placeTextareaCaret', () => {
  function box() {
    const el = makeTextarea({ value: 'a\nb\nc', selectionStart: 2, selectionEnd: 3 });
    Object.assign(el, { scrollTop: 40, scrollHeight: 120 });
    return el;
  }

  it('collapses the caret at the start and scrolls to the top', () => {
    const el = box();
    placeTextareaCaret(el, 'start');
    expect([el.selectionStart, el.selectionEnd, el.scrollTop]).toEqual([0, 0, 0]);
  });

  it('collapses the caret at the end and scrolls to the bottom', () => {
    const el = box();
    placeTextareaCaret(el, 'end');
    expect([el.selectionStart, el.selectionEnd, el.scrollTop]).toEqual([5, 5, 120]);
  });
});
