import { describe, it, expect } from 'vitest';
import { filterDropdownOptions, isTypeaheadSeedKey, type DropdownOption } from './Dropdown';

const opts: DropdownOption[] = [
  { value: 'lucidos-agent', label: 'Lucidos Agent' },
  { value: 'src', label: 'Lucidos source' },
  { value: 'habit', label: 'Habit Tracker · app' },
  { value: 'demo', label: 'Demo Director · app' },
];

describe('filterDropdownOptions (type-to-search)', () => {
  it('returns the full list for an empty / whitespace query', () => {
    expect(filterDropdownOptions(opts, '')).toBe(opts);
    expect(filterDropdownOptions(opts, '   ')).toBe(opts);
  });

  it('matches by case-insensitive label substring', () => {
    expect(filterDropdownOptions(opts, 'habit').map(o => o.value)).toEqual(['habit']);
    expect(filterDropdownOptions(opts, 'DEMO').map(o => o.value)).toEqual(['demo']);
    expect(filterDropdownOptions(opts, 'app').map(o => o.value)).toEqual(['habit', 'demo']);
  });

  it('matches a mid-label substring, not just a prefix', () => {
    expect(filterDropdownOptions(opts, 'source').map(o => o.value)).toEqual(['src']);
  });

  it('returns an empty list when nothing matches (renders "No matches")', () => {
    expect(filterDropdownOptions(opts, 'zzz')).toEqual([]);
  });
});

describe('isTypeaheadSeedKey (when a keystroke starts type-to-search)', () => {
  const key = (over: Partial<{ key: string; metaKey: boolean; ctrlKey: boolean; altKey: boolean }> = {}) => ({
    key: 'a', metaKey: false, ctrlKey: false, altKey: false, ...over,
  });

  it('seeds on a bare printable char when not yet searching', () => {
    expect(isTypeaheadSeedKey(key(), { freeText: false, searching: false })).toBe(true);
  });

  it('does NOT seed once already searching — the focused filter box owns input', () => {
    expect(isTypeaheadSeedKey(key(), { freeText: false, searching: true })).toBe(false);
  });

  it('never seeds for a freeText dropdown (its trigger is the input)', () => {
    expect(isTypeaheadSeedKey(key(), { freeText: true, searching: false })).toBe(false);
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
