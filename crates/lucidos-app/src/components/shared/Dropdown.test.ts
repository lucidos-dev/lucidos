import { describe, it, expect } from 'vitest';
import {
  dropdownMenuClass,
  dropdownPanelStyle,
  filterDropdownOptions,
  isTypeaheadSeedKey,
  type DropdownOption,
} from './Dropdown';

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

describe('dropdownMenuClass (the portaled menu carries its own context)', () => {
  /** A trigger whose `closest` matches exactly the given selectors. */
  const trigger = (...matches: string[]) => ({
    closest: (selector: string) => (matches.includes(selector) ? {} : null),
  });

  it('is the plain menu class for a trigger outside any form group', () => {
    expect(dropdownMenuClass(trigger())).toBe('dropdown-menu');
  });

  it('adds the field class for a trigger inside a .form-group', () => {
    // The menu is portaled to <body>, so `.form-group .dropdown-option` can no
    // longer reach it through the DOM: the context has to ride on the panel.
    expect(dropdownMenuClass(trigger('.form-group'))).toBe('dropdown-menu dropdown-menu-field');
  });

  it('is the plain menu class before the wrapper has mounted (no trigger yet)', () => {
    expect(dropdownMenuClass(null)).toBe('dropdown-menu');
  });
});

describe('dropdownPanelStyle (the panel is measured before it is placed)', () => {
  it('carries the trigger width BEFORE a position exists, so the measurement is honest', () => {
    // The panel is portaled to <body>, where the stylesheet's `min-width: 100%`
    // resolves against the initial containing block. Without an inline
    // minWidth the first measurement reports a viewport-wide menu and the
    // computed `left` strands at the viewport margin instead of the trigger.
    const style = dropdownPanelStyle(180, null);
    expect(style.minWidth).toBe('180px');
    expect(style.visibility).toBe('hidden');
    // Fixed + zeroed offsets keep the hidden box in the viewport rather than
    // 100vh down the document (the stylesheet's `top: calc(100% + 0.25rem)`).
    expect(style.position).toBe('fixed');
    expect(style.top).toBe('0px');
    expect(style.left).toBe('0px');
  });

  it('places the panel at the computed offsets once measured, and reveals it', () => {
    const style = dropdownPanelStyle(180, { top: 42, left: 96 });
    expect(style).toMatchObject({
      position: 'fixed',
      top: '42px',
      left: '96px',
      minWidth: '180px',
    });
    expect(style.visibility).toBeUndefined();
  });

  it('stays hidden with no anchor at all', () => {
    expect(dropdownPanelStyle(null, { top: 1, left: 2 })).toEqual({ visibility: 'hidden' });
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
