import { describe, expect, it } from 'vitest';
import { liftTarget, titleLines, type Box } from '../agentMarkLift';

const box = (left: number, right: number, top: number, bottom: number): Box => ({ left, right, top, bottom });
const COLUMN = 280;
const GAP = 8;

/** Two wrapped lines of 20px, the second ending at x=120. */
const WRAPPED = [box(0, 250, 0, 20), box(0, 120, 20, 40)];
/** The pin and ⋯ at the first line's right end. */
const ACTIONS_ON_FIRST_LINE = box(230, 300, 0, 20);

describe('titleLines', () => {
    it('reads a DOMRect, whose edges are prototype getters a spread would drop', () => {
        class Rect {
            get left() { return 0; }
            get right() { return 250; }
            get top() { return 0; }
            get bottom() { return 20; }
        }
        expect(titleLines([new Rect()])).toEqual([box(0, 250, 0, 20)]);
    });

    it('joins the pieces of one line and splits the next', () => {
        const withIcon = [box(0, 14, 3, 17), box(18, 250, 0, 20), box(0, 120, 20, 40)];
        expect(titleLines(withIcon)).toEqual([box(0, 250, 0, 20), box(0, 120, 20, 40)]);
    });
});

describe('liftTarget', () => {
    it('rises onto the last line of a wrapped title that ends left of the column', () => {
        expect(liftTarget(WRAPPED, ACTIONS_ON_FIRST_LINE, COLUMN, GAP)).toBe(30);
    });

    it('stays put when the last line reaches the column', () => {
        const long = [box(0, 250, 0, 20), box(0, 275, 20, 40)];
        expect(liftTarget(long, ACTIONS_ON_FIRST_LINE, COLUMN, GAP)).toBeNull();
    });

    it('never rises onto a one-line desktop title, whose line the actions own', () => {
        expect(liftTarget([box(0, 100, 0, 20)], ACTIONS_ON_FIRST_LINE, COLUMN, GAP)).toBeNull();
    });

    it('rises onto a short one-line title where no actions draw', () => {
        expect(liftTarget([box(0, 100, 0, 20)], null, COLUMN, GAP)).toBe(10);
    });

    it('has nothing to lift onto without a title', () => {
        expect(liftTarget([], null, COLUMN, GAP)).toBeNull();
    });
});
