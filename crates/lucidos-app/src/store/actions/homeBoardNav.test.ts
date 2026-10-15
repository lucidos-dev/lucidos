/**
 * The Home icon opens the Home chat; on desktop the board lives in its rail.
 * The phone keeps one pane with two faces.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('./homeThread', () => ({ openHomeThread: vi.fn() }));
vi.mock('../homeBoard', () => ({ landNextHomeOnBoard: vi.fn() }));

import { openHome, revealHomeBoard } from './homeBoardNav';
import { openHomeThread } from './homeThread';
import { landNextHomeOnBoard } from '../homeBoard';
import { homeRailShown, setHomeRailShown } from '../homeRail';
import { activeMenuItem, focusedThreadId } from '../store';
import { viewportIsMobile } from '../../utils/viewport';

describe('openHome', () => {
  const wasMobile = viewportIsMobile.value;

  beforeEach(() => {
    vi.clearAllMocks();
    activeMenuItem.value = 'files';
  });

  afterEach(() => {
    viewportIsMobile.value = wasMobile;
  });

  it('on desktop opens the Home chat and leaves the Canvas alone', () => {
    viewportIsMobile.value = false;
    openHome();
    expect(openHomeThread).toHaveBeenCalledTimes(1);
    expect(activeMenuItem.value).toBe('files');
    expect(landNextHomeOnBoard).not.toHaveBeenCalled();
  });

  it('on desktop in Home, leaves a hidden rail hidden: only its toggle shows it', () => {
    viewportIsMobile.value = false;
    setHomeRailShown(false);
    focusedThreadId.value = 'home-1';
    openHome();
    expect(homeRailShown.value).toBe(false);
    expect(openHomeThread).toHaveBeenCalledTimes(1);
    focusedThreadId.value = null;
  });

  it('on a phone lands Home on its board face', () => {
    viewportIsMobile.value = true;
    openHome();
    expect(landNextHomeOnBoard).toHaveBeenCalledTimes(1);
    expect(openHomeThread).toHaveBeenCalledTimes(1);
  });
});

describe('revealHomeBoard', () => {
  const wasMobile = viewportIsMobile.value;

  beforeEach(() => {
    vi.clearAllMocks();
    activeMenuItem.value = 'files';
    setHomeRailShown(false);
  });

  afterEach(() => {
    viewportIsMobile.value = wasMobile;
  });

  it('on desktop shows the rail and leaves the Canvas and the Home chat alone', () => {
    viewportIsMobile.value = false;
    revealHomeBoard();
    expect(homeRailShown.value).toBe(true);
    expect(activeMenuItem.value).toBe('files');
    expect(openHomeThread).not.toHaveBeenCalled();
  });

  it("on a phone lands on Home's board face", () => {
    viewportIsMobile.value = true;
    revealHomeBoard();
    expect(landNextHomeOnBoard).toHaveBeenCalledTimes(1);
    expect(openHomeThread).toHaveBeenCalledTimes(1);
  });
});
