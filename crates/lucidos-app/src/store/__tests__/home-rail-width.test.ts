/**
 * The desktop rail that holds the Home board: its floor is two board cells, a
 * drag stops at both walls, and the width and shown state persist per device.
 * The width it draws at is `computeDrawnLayout`'s (splitHelpers.test.ts).
 */
import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
import { BOARD_CELL_REM, BOARD_GAP_REM, BOARD_PAD_X_REM, boardColumns, boardContentWidth } from '../../utils/boardGeometry';
import { homeRailCeiling } from '../../components/layout/splitHelpers';
import { DIVIDER_BAND_REM, computeMinHomeRailWidth } from '../paneMinimums';
import { HOME_RAIL_SHOWN_KEY, HOME_RAIL_WIDTH_KEY } from '../homeRailKeys';
import { homeRailShown, homeRailWidth, persistHomeRailWidth, setHomeRailShown } from '../homeRail';

describe('the rail floor', () => {
  it('fits exactly two board cells at every UI scale, beside any scrollbar gutter', () => {
    for (const rem of [16, 18, 20, 24, 28]) {
      for (const gutter of [0, 8, 15]) {
        const floor = computeMinHomeRailWidth(rem, gutter);
        const content = boardContentWidth(floor - DIVIDER_BAND_REM * rem - gutter, rem);
        expect(boardColumns(content, rem)).toBe(2);
        expect(content).toBeGreaterThanOrEqual((2 * BOARD_CELL_REM + BOARD_GAP_REM) * rem);
        expect(content).toBeLessThan((2 * BOARD_CELL_REM + BOARD_GAP_REM) * rem + 1);
      }
    }
  });

  it('grows with the root font size', () => {
    expect(computeMinHomeRailWidth(20, 0)).toBeGreaterThan(computeMinHomeRailWidth(16, 0));
  });

  it('keeps the rail body\'s scrollbar gutter reserved, which the floor counts', () => {
    const css = readFileSync(new URL('../../styles/panels/home-board.css', import.meta.url), 'utf8');
    expect(/\.home-rail-body \{[^}]*scrollbar-gutter: stable/.test(css)).toBe(true);
  });

  it("mirrors the board's side padding in CSS", () => {
    const css = readFileSync(new URL('../../styles/panels/home-board.css', import.meta.url), 'utf8');
    const board = /\.home-board \{[^}]*padding: var\(--space-md\) var\(--space-lg\)/.exec(css);
    expect(board, '.home-board pads its sides with --space-lg, which BOARD_PAD_X_REM restates').not.toBeNull();
    expect(BOARD_PAD_X_REM).toBe(1);
  });

  it("mirrors the divider band the rail pads its leading side by", () => {
    const rail = readFileSync(new URL('../../styles/panels/home-board.css', import.meta.url), 'utf8');
    expect(/\.home-rail \{[^}]*padding-left: var\(--divider-width\)/.test(rail)).toBe(true);
    const base = readFileSync(new URL('../../styles/global/base.css', import.meta.url), 'utf8');
    expect(base).toContain(`--divider-width: ${DIVIDER_BAND_REM}rem;`);
  });
});

describe('homeRailCeiling', () => {
  const bounds = { minThreadPx: 400, minContentPx: 360 };

  it('takes a wider rail out of the Canvas while the Conversation holds its width', () => {
    expect(homeRailCeiling(1600, 500, 0.4, bounds)).toBe(1600 - 500 - 360);
  });

  it('takes it out of the one pane left when the other is collapsed', () => {
    expect(homeRailCeiling(1600, 0, 0, bounds)).toBe(1600 - 360);
    expect(homeRailCeiling(1600, 1200, 1, bounds)).toBe(1600 - 400);
  });
});

describe('the rail store', () => {
  it('persists the shown state per device', () => {
    const before = homeRailShown.value;
    setHomeRailShown(!before);
    expect(localStorage.getItem(HOME_RAIL_SHOWN_KEY)).toBe(String(!before));
    setHomeRailShown(before);
    expect(homeRailShown.value).toBe(before);
  });

  it('persists the width', () => {
    homeRailWidth.value = 512;
    persistHomeRailWidth();
    expect(localStorage.getItem(HOME_RAIL_WIDTH_KEY)).toBe('512');
  });
});
