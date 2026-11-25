import { describe, it, expect, beforeEach, vi } from 'vitest';
import { focusedPane, splitRatio } from '../store';
import { markNavigationStart } from '../../utils/navigationMarks';
import { revealContentPane, revealThreadPane } from './pane';

vi.mock('../../utils/navigationMarks', async (importActual) => ({
  ...(await importActual<typeof import('../../utils/navigationMarks')>()),
  markNavigationStart: vi.fn(),
}));

const stamp = vi.mocked(markNavigationStart);

describe('desktop thread reveal records no navigation mark', () => {
  beforeEach(() => {
    (globalThis as any).innerWidth = 1024;
    focusedPane.value = 'content';
    splitRatio.value = 0.5;
    stamp.mockClear();
  });

  it('a desktop reveal of the thread pane leaves no mark for a later span to inherit', () => {
    revealThreadPane();
    expect(stamp).not.toHaveBeenCalled();
  });

  it('a desktop reveal of the content pane still records its mark', () => {
    revealContentPane();
    expect(stamp.mock.calls[0]?.[0]).toBe('content');
  });
});
