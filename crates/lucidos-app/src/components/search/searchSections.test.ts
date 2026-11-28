import { describe, expect, it } from 'vitest';
import type { SearchResultItem, TextSearchHit } from '../../api/client';
import { rankedSections, textHitItem } from './searchSections';

function hit(category: string, title: string): SearchResultItem {
  return { id: title, title, subtitle: '', category, score: 1 };
}

function line(path: string, lineNo: number, text: string): TextSearchHit {
  return { path, line: lineNo, before: '', matched: text, after: '' };
}

describe('sections that answer late on the All tab', () => {
  const settings = [hit('settings', 'Open links in the in-app browser')];
  const threads = [hit('threads', 'iOS PWA External Browser Fix')];

  it('rank by their best hit when all answer together', () => {
    expect(rankedSections({ settings, threads }, 'browser').map(s => s.section)).toEqual(['threads', 'settings']);
  });

  it('slot below the sections already drawn, even when they rank higher', () => {
    const sections = rankedSections({ settings, threads }, 'browser', [], ['settings']);
    expect(sections.map(s => s.section)).toEqual(['settings', 'threads']);
    expect(sections[1].offset).toBe(1);
  });

  it('keep their own rank among each other', () => {
    const files = [hit('files', 'notes-on-browser-support-and-more.md')];
    const sections = rankedSections({ settings, files, threads }, 'browser', [], ['settings']);
    expect(sections.map(s => s.section)).toEqual(['settings', 'threads', 'files']);
  });

  it('leave out a drawn section that has no hits now', () => {
    expect(rankedSections({ threads }, 'browser', [], ['settings']).map(s => s.section)).toEqual(['threads']);
  });
});

describe('the Text section on the All tab', () => {
  it('comes after every name section, even when its line is the query exactly', () => {
    const sections = rankedSections(
      { files: [hit('files', 'my-roadmap-notes.md')], apps: [hit('apps', 'Planner')] },
      'roadmap',
      [textHitItem(line('knowhow/plan.md', 3, 'roadmap'))],
    );
    expect(sections.map(s => s.section)).toEqual(['files', 'apps', 'text']);
  });

  it('continues the keyboard offsets after the name sections', () => {
    const lines = [1, 2].map(n => textHitItem(line('a.md', n, 'x')));
    const sections = rankedSections({ files: [hit('files', 'x.md')] }, 'x', lines);
    const text = sections[sections.length - 1];
    expect(text.items).toEqual(lines);
    expect(text.offset).toBe(1);
  });

  it('is left out when no line matched', () => {
    expect(rankedSections({ files: [hit('files', 'x.md')] }, 'x', []).map(s => s.section)).toEqual(['files']);
  });

  it('keys a line by its path and line number', () => {
    expect(textHitItem(line('apps/demo/app.js', 12, 'x')).id).toBe('apps/demo/app.js:12');
  });
});
