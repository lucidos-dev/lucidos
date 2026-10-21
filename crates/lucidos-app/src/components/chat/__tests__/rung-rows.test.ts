import { describe, it, expect } from 'vitest';
import {
  LADDER_LINK_VALUE, NO_ESTIMATE_NOTE, NOT_ESTIMATED_SECTION, OFF_LADDER_NOTE, RUNG_SECTION, rungListRows, rungRowPair, type RungListInput,
} from '../rungRows';
import type { CodingAgentLadderRung } from '../../../api/client';
import { encodePair } from '../../../store/modelSelection';

const band = (label: string) => ({ multiple: 1, label });
const rungs: CodingAgentLadderRung[] = [
  {
    model: 'claude-haiku-5-5', effort: 'medium', label: 'Haiku 5.5',
    intelligence: band('0.5×'), cost: band('0.2×'), time: band('0.4×'), estimated: true,
  },
  {
    model: 'claude-opus-5-6', effort: 'medium', label: 'Opus 5.6',
    intelligence: band('0.84×'), cost: band('included'), time: null, estimated: true,
  },
  { model: 'claude-opus-5-6', effort: 'high', label: 'Opus 5.6', intelligence: null, cost: null, time: null, estimated: true },
];

/** The picker as the engine serves it: Claude Code also lists old models and
 *  the 1M twin, and none of them may reach the rung list. */
const options = [
  { value: 'default', label: 'Default (recommended)', description: '', resolved_model: 'claude-opus-5-6[1m]' },
  { value: 'opus', label: 'Opus', description: '', resolved_model: 'claude-opus-5-6[1m]' },
  { value: 'claude-opus-5-6[1m]', label: 'Opus 5.6 (1M)', description: '' },
  { value: 'claude-opus-4-8', label: 'Opus 4.8', description: '' },
];

const input = (over: Partial<RungListInput> = {}): RungListInput => ({
  rungs,
  options,
  model: null,
  effort: null,
  modelLabel: (m) => m,
  defaultPair: { model: 'claude-opus-5-6', effort: 'medium' },
  ...over,
});

describe('rungListRows', () => {
  it('describes a rung by its bands, so two rungs of one effort read apart', () => {
    const notes = rungListRows(input()).rows.map((r) => r.description);
    expect(notes.slice(0, 3)).toEqual([
      'Intelligence 0.5× · cost 0.2× · time 0.4×',
      'Intelligence 0.84× · cost included',
      NO_ESTIMATE_NOTE,
    ]);
  });

  it('lists the on rungs and the ladder link, last, and nothing else', () => {
    const { rows } = rungListRows(input());
    expect(rows.map((r) => r.label)).toEqual([
      'Haiku 5.5 · medium', 'Opus 5.6 · medium', 'Opus 5.6 · high', 'Edit the intelligence ladder',
    ]);
    expect(rows.map((r) => r.label).join(' ')).not.toMatch(/4\.|1M|Default/);
    expect(rows[rows.length - 1].value).toBe(LADDER_LINK_VALUE);
  });

  it('gives a rung the value of its whole pair, so one pick commits both', () => {
    const row = rungListRows(input()).rows.find((r) => r.label === 'Opus 5.6 · high')!;
    expect(row.value).toBe(encodePair('claude-opus-5-6', 'high'));
    expect(rungRowPair(row.value)).toEqual({ model: 'claude-opus-5-6', effort: 'high' });
  });

  it('commits nothing for the ladder link', () => {
    expect(rungRowPair(LADDER_LINK_VALUE)).toBeNull();
  });

  it('checks the default rung, in its own place, while nothing is picked or recorded', () => {
    const checked = encodePair('claude-opus-5-6', 'medium');
    expect(rungListRows(input()).current).toBe(checked);
    expect(rungListRows(input({ model: 'default' })).current).toBe(checked);
  });

  it('checks no row while nothing is picked and no default is known', () => {
    expect(rungListRows(input({ defaultPair: null })).current).toBe('');
  });

  it('adds a default pair the ladder does not hold, marked, before the ladder link', () => {
    const { rows, current } = rungListRows(input({
      defaultPair: { model: 'claude-opus-4-8', effort: 'high' }, modelLabel: () => 'Opus 4.8',
    }));
    const own = rows[rows.length - 2];
    expect(own.label).toBe('Opus 4.8 · high');
    expect(own.description).toBe(OFF_LADDER_NOTE);
    expect(current).toBe(own.value);
  });

  it('prefers a picked pair over the default', () => {
    expect(rungListRows(input({ model: 'claude-opus-5-6', effort: 'high' })).current)
      .toBe(encodePair('claude-opus-5-6', 'high'));
  });

  it('checks the rung a thread runs, through an alias and a 1M twin', () => {
    const viaAlias = rungListRows(input({ model: 'opus', effort: 'high' }));
    expect(viaAlias.current).toBe(encodePair('claude-opus-5-6', 'high'));
    const viaTwin = rungListRows(input({ model: 'claude-opus-5-6[1m]', effort: 'medium' }));
    expect(viaTwin.current).toBe(encodePair('claude-opus-5-6', 'medium'));
    expect(viaTwin.rows).toHaveLength(4);
  });

  it("keeps a thread's own off-ladder pair, marked, before the ladder link", () => {
    const { rows, current } = rungListRows(input({
      model: 'claude-opus-4-8', effort: 'high', modelLabel: () => 'Opus 4.8',
    }));
    const own = rows[rows.length - 2];
    expect(own.label).toBe('Opus 4.8 · high');
    expect(own.description).toBe(OFF_LADDER_NOTE);
    expect(current).toBe(own.value);
  });

  it('treats a known model at an effort off the ladder as off the ladder', () => {
    const { rows } = rungListRows(input({ model: 'claude-opus-5-6', effort: 'low', modelLabel: () => 'Opus 5.6' }));
    expect(rows[rows.length - 2].description).toBe(OFF_LADDER_NOTE);
  });

  describe('with unestimated rungs', () => {
    const mixed = [
      { model: 'my-model', effort: 'high', label: 'My Model', intelligence: null, cost: null, time: null, estimated: false },
      ...rungs,
      { model: 'other-model', effort: 'low', label: 'Other Model', intelligence: null, cost: null, time: null, estimated: false },
    ];

    it('lists them last, in ladder order, under the not-estimated section', () => {
      const { rows } = rungListRows(input({ rungs: mixed }));
      expect(rows.map((r) => r.label)).toEqual([
        'Haiku 5.5 · medium', 'Opus 5.6 · medium', 'Opus 5.6 · high',
        'My Model · high', 'Other Model · low', 'Edit the intelligence ladder',
      ]);
      const sections = rows.map((r) => r.section);
      expect(sections.slice(0, 3).every((s) => s === RUNG_SECTION)).toBe(true);
      expect(sections.slice(3, 5)).toEqual([NOT_ESTIMATED_SECTION, NOT_ESTIMATED_SECTION]);
    });

    it("keeps the thread's off-ladder pair in the main section, before them", () => {
      const { rows } = rungListRows(input({
        rungs: mixed, model: 'claude-opus-4-8', effort: 'high', modelLabel: () => 'Opus 4.8',
      }));
      const labels = rows.map((r) => r.label);
      expect(labels.indexOf('Opus 4.8 · high')).toBeLessThan(labels.indexOf('My Model · high'));
      expect(rows[labels.indexOf('Opus 4.8 · high')].section).toBe(RUNG_SECTION);
    });

    it('checks an unestimated rung the thread runs', () => {
      const { current } = rungListRows(input({ rungs: mixed, model: 'my-model', effort: 'high' }));
      expect(current).toBe(encodePair('my-model', 'high'));
    });
  });
});
