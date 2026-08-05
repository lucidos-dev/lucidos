import { describe, it, expect, beforeEach } from 'vitest';
import { normalizeLineRange, consumeLineScrollTarget, lineScrollTarget } from '../store';

// `line` / `line_end` arrive from outside the app (an app iframe's
// lucidos.ui.navigate, an LLM navigate_ui, an <a href> inside a previewed
// artifact), so anything that isn't a positive whole number must be refused
// rather than turned into a row index.
describe('normalizeLineRange', () => {
  it('turns a lone line into a single-line range', () => {
    expect(normalizeLineRange(510)).toEqual({ start: 510, end: 510 });
  });

  it('keeps a well-formed range', () => {
    expect(normalizeLineRange(10, 20)).toEqual({ start: 10, end: 20 });
  });

  it('swaps an inverted range rather than dropping it', () => {
    expect(normalizeLineRange(20, 10)).toEqual({ start: 10, end: 20 });
  });

  it('accepts the first line', () => {
    expect(normalizeLineRange(1, 1)).toEqual({ start: 1, end: 1 });
  });

  it('returns null when no line is given, so a plain navigate opens at the top', () => {
    expect(normalizeLineRange(undefined)).toBeNull();
    expect(normalizeLineRange(undefined, 20)).toBeNull();
    expect(normalizeLineRange(null)).toBeNull();
  });

  it('rejects a line that could not name a row', () => {
    expect(normalizeLineRange(0)).toBeNull();
    expect(normalizeLineRange(-3)).toBeNull();
    expect(normalizeLineRange(1.5)).toBeNull();
    expect(normalizeLineRange(NaN)).toBeNull();
    expect(normalizeLineRange(Infinity)).toBeNull();
    expect(normalizeLineRange('12')).toBeNull();
    expect(normalizeLineRange({ start: 1 })).toBeNull();
  });

  it('ignores an unusable line_end instead of losing the line itself', () => {
    expect(normalizeLineRange(10, 0)).toEqual({ start: 10, end: 10 });
    expect(normalizeLineRange(10, -1)).toEqual({ start: 10, end: 10 });
    expect(normalizeLineRange(10, 'abc')).toEqual({ start: 10, end: 10 });
  });

  // A range past the end of the file is deliberately NOT rejected here: the
  // line count isn't known until the content loads, and a stale citation must
  // still open its file.
  it('accepts a range the file may be too short for', () => {
    expect(normalizeLineRange(9_000_000)).toEqual({ start: 9_000_000, end: 9_000_000 });
  });
});

describe('consumeLineScrollTarget', () => {
  beforeEach(() => {
    lineScrollTarget.value = null;
  });

  it('returns nothing when no scroll was requested', () => {
    expect(consumeLineScrollTarget()).toBeNull();
    expect(lineScrollTarget.value).toBeNull();
  });

  // One-shot: a re-render must not re-scroll a user who has since scrolled away.
  it('hands the target over once and clears it', () => {
    lineScrollTarget.value = 510;

    expect(consumeLineScrollTarget()).toBe(510);
    expect(lineScrollTarget.value).toBeNull();
    expect(consumeLineScrollTarget()).toBeNull();
  });
});
