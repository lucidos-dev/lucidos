import { describe, it, expect } from 'vitest';
import { bestFirst, rankByTitle, titleRank, type TitleRank } from './titleMatch';
import fixture from '../../generated/title-match-fixture.json';

interface FixtureCase {
  title: string;
  query: string;
  expected: TitleRank;
}

describe('titleRank agrees with the engine', () => {
  it.each(fixture as FixtureCase[])('"$title" for "$query"', ({ title, query, expected }) => {
    const rank = titleRank(title, query);
    expect(rank.level).toBe(expected.level);
    expect(rank.coverage).toBeCloseTo(expected.coverage, 12);
  });
});

describe('bestFirst', () => {
  it('puts a stronger level first whatever the coverage', () => {
    const wordStart = titleRank('A very long title naming settings somewhere', 'settings');
    const phrase = titleRank('xsettings', 'settings');
    expect(bestFirst(wordStart, phrase)).toBeLessThan(0);
  });

  it('breaks a tie within a level on coverage', () => {
    const shortcut = titleRank('Open settings (⌘,)', 'settings');
    const file = titleRank('settings-system-v2.png', 'settings');
    expect(bestFirst(shortcut, file)).toBeLessThan(0);
  });
});

describe('rankByTitle', () => {
  it('keeps the given order among equal ranks', () => {
    const items = [{ title: 'Habit Tracker' }, { title: 'Backup' }, { title: 'Settings' }];
    expect(rankByTitle(items, 'settings', i => i.title).map(i => i.title)).toEqual(['Settings', 'Habit Tracker', 'Backup']);
  });
});
