import { describe, expect, it } from 'vitest';
import fixture from '../../generated/spoken-merge-fixture.json';
import { MERGE_GAP_SECS, isOneUtterance, joinSpoken } from './spokenMerge';

type MergeCase = {
  name: string;
  first: string;
  second: string;
  gap_secs: number;
  same_speaker: boolean;
  merges: boolean;
  joined: string;
};

const cases = fixture.cases as MergeCase[];

describe('the spoken-merge rule matches the engine', () => {
  it('agrees on the gap bound itself', () => {
    expect(fixture.merge_gap_secs).toBe(MERGE_GAP_SECS);
  });

  it('has cases to replay', () => {
    expect(cases.length).toBeGreaterThan(0);
  });

  it.each(cases)('$name', (c) => {
    expect(isOneUtterance(c.gap_secs, c.same_speaker)).toBe(c.merges);
    expect(joinSpoken(c.first, c.second)).toBe(c.joined);
  });
});
