import { describe, it, expect } from 'vitest';
import { withPreviewRevision } from './previewRevision';

describe('withPreviewRevision', () => {
  const DATA = '/data/artifacts/clips/demo.mp4';
  const REPO = '/api/v1/repositories/r1/file?path=docs%2Fdiagram.png&ref=main';

  it('returns the URL byte for byte at revision zero', () => {
    expect(withPreviewRevision(DATA, 0)).toBe(DATA);
    expect(withPreviewRevision(REPO, 0)).toBe(REPO);
  });

  it('opens a query on a URL that has none', () => {
    expect(withPreviewRevision(DATA, 2)).toBe(`${DATA}?v=2`);
  });

  it('extends a query the URL already has', () => {
    expect(withPreviewRevision(REPO, 2)).toBe(`${REPO}&v=2`);
  });

  it('extends the app-local data URL that carries a WIP thread id', () => {
    const wip = '/app/habit-tracker/img/chart.png?thread_id=t1';
    expect(withPreviewRevision(wip, 4)).toBe(`${wip}&v=4`);
  });
});
