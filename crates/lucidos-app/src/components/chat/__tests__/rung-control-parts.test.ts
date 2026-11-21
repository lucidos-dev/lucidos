import { describe, it, expect } from 'vitest';
import { shownRung, stepTargets } from '../rungControlParts';
import { LADDER_LINK_VALUE } from '../rungRows';

const rungs = [
  { model: 'claude-opus-5-6', effort: 'medium', label: 'Opus 5.6', intelligence: null, cost: null, time: null, estimated: true },
  { model: 'claude-opus-5-6', effort: 'high', label: 'Opus 5.6', intelligence: { multiple: 1.08, label: '1.08×' }, cost: null, time: null, estimated: true },
];

const options = [
  { value: 'default', label: 'Default (recommended)', description: '', resolved_model: 'claude-opus-5-6[1m]' },
  { value: 'opus', label: 'Opus', description: '', resolved_model: 'claude-opus-5-6[1m]' },
];

const base = {
  rungs,
  options,
  model: null as string | null,
  effort: null as string | null,
  defaultPair: { model: 'claude-opus-5-6', effort: 'medium' as string | null },
  agentName: 'Codex',
  modelLabel: (m: string) => `label:${m}`,
};

describe('shownRung', () => {
  it('shows nothing until the commands have loaded', () => {
    expect(shownRung({ ...base, loaded: false })).toBeNull();
  });

  it('names the default implementer while nothing is picked', () => {
    expect(shownRung({ ...base, loaded: true })).toEqual({ name: 'Opus 5.6', effort: 'medium', position: { at: 1, of: 2 }, intelligence: null });
  });

  it('names the backend, with no effort, when nothing is picked and no default is set', () => {
    expect(shownRung({ ...base, loaded: true, defaultPair: null })).toEqual({ name: 'Codex', effort: null, position: null, intelligence: null });
  });

  it('names a picked rung by its ladder label and band, through an alias', () => {
    expect(shownRung({ ...base, loaded: true, model: 'opus', effort: 'high' }))
      .toEqual({ name: 'Opus 5.6', effort: 'high', position: { at: 2, of: 2 }, intelligence: '1.08×' });
  });

  it('names an off-ladder pair by its model label', () => {
    expect(shownRung({ ...base, loaded: true, model: 'claude-opus-4-8', effort: 'low' }))
      .toEqual({ name: 'label:claude-opus-4-8', effort: 'low', position: null, intelligence: null });
  });

  it('treats the default sentinel as nothing picked', () => {
    expect(shownRung({ ...base, loaded: true, model: 'default' }))
      .toEqual({ name: 'Opus 5.6', effort: 'medium', position: { at: 1, of: 2 }, intelligence: null });
  });
});

describe('stepTargets', () => {
  const rows = [
    { value: 'default', label: 'Default' },
    { value: 'a', label: 'A' },
    { value: 'b', label: 'B' },
    { value: LADDER_LINK_VALUE, label: 'Edit the intelligence ladder' },
  ];

  it('steps to the row above and the row below', () => {
    expect(stepTargets(rows, 'a')).toEqual({ prev: 'default', next: 'b' });
  });

  it('stops at the first row, with nothing above it', () => {
    expect(stepTargets(rows, 'default')).toEqual({ prev: null, next: 'a' });
  });

  it('stops at the last rung, and never lands on the ladder link', () => {
    expect(stepTargets(rows, 'b')).toEqual({ prev: 'a', next: null });
  });

  it('offers no step from a row the list does not hold', () => {
    expect(stepTargets(rows, 'gone')).toEqual({ prev: null, next: null });
  });
});
