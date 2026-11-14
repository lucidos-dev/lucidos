import { describe, it, expect } from 'vitest';
import { isTypeaheadKey, isTypeaheadSeedKey } from './typeahead';

const key = (over: Partial<{ key: string; metaKey: boolean; ctrlKey: boolean; altKey: boolean }> = {}) => ({
  key: 'a', metaKey: false, ctrlKey: false, altKey: false, ...over,
});

describe('isTypeaheadKey (what types into a filter box)', () => {
  it('takes a bare printable char', () => {
    expect(isTypeaheadKey(key())).toBe(true);
  });

  it('leaves Space and every non-printable key alone', () => {
    expect(isTypeaheadKey(key({ key: ' ' }))).toBe(false);
    expect(isTypeaheadKey(key({ key: 'Backspace' }))).toBe(false);
  });

  it('leaves a modifier chord alone, which is a shortcut', () => {
    expect(isTypeaheadKey(key({ metaKey: true }))).toBe(false);
  });
});

describe('isTypeaheadSeedKey (when a keystroke starts type-to-search)', () => {

  it('seeds on a bare printable char when not yet searching', () => {
    expect(isTypeaheadSeedKey(key(), { freeText: false, searching: false })).toBe(true);
  });

  it('does NOT seed once already searching: the focused filter box owns input', () => {
    expect(isTypeaheadSeedKey(key(), { freeText: false, searching: true })).toBe(false);
  });

  it('never seeds for a freeText dropdown (its trigger is the input)', () => {
    expect(isTypeaheadSeedKey(key(), { freeText: true, searching: false })).toBe(false);
  });

  it('takes freeText as optional, for a caller with no such trigger', () => {
    expect(isTypeaheadSeedKey(key(), { searching: false })).toBe(true);
  });

  it('ignores Space (handled separately as open/no-op) and non-printable keys', () => {
    expect(isTypeaheadSeedKey(key({ key: ' ' }), { freeText: false, searching: false })).toBe(false);
    expect(isTypeaheadSeedKey(key({ key: 'Enter' }), { freeText: false, searching: false })).toBe(false);
    expect(isTypeaheadSeedKey(key({ key: 'ArrowDown' }), { freeText: false, searching: false })).toBe(false);
    expect(isTypeaheadSeedKey(key({ key: 'Tab' }), { freeText: false, searching: false })).toBe(false);
  });

  it('ignores modifier chords (those are shortcuts, not search)', () => {
    expect(isTypeaheadSeedKey(key({ metaKey: true }), { freeText: false, searching: false })).toBe(false);
    expect(isTypeaheadSeedKey(key({ ctrlKey: true }), { freeText: false, searching: false })).toBe(false);
    expect(isTypeaheadSeedKey(key({ altKey: true }), { freeText: false, searching: false })).toBe(false);
  });
});
