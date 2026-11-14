import { describe, expect, it } from 'vitest';
import { stackLiftTarget, titleLines, type Box } from '../agentMarkLift';

const box = (left: number, right: number, top: number, bottom: number): Box => ({ left, right, top, bottom });
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

// The marks keep one stack gap instead of following a text baseline.
describe('stackLiftTarget', () => {
    const bottom = box(280, 296, 48, 64);

    it('puts the top mark one stack gap above the lower mark', () => {
        expect(stackLiftTarget(WRAPPED, ACTIONS_ON_FIRST_LINE, bottom, 4, GAP)).toBe(36);
    });

    it('keeps clear space below the pin, including at the boundary', () => {
        expect(stackLiftTarget(WRAPPED, box(280, 300, 0, 21), bottom, 4, GAP)).toBeNull();
        expect(stackLiftTarget(WRAPPED, box(280, 300, 0, 20), bottom, 4, GAP)).toBe(36);
    });

    it('rejects a mark that would overlap an earlier title line', () => {
        const lines = [box(0, 290, 0, 30), box(0, 120, 30, 40)];
        expect(stackLiftTarget(lines, null, bottom, 4, GAP)).toBeNull();
    });

    it('keeps clear of a last title line that reaches the column', () => {
        expect(stackLiftTarget([box(0, 290, 20, 40)], null, bottom, 4, GAP)).toBeNull();
    });

    it('lifts beside a one-line title whose actions sit well above', () => {
        expect(stackLiftTarget([box(0, 100, 0, 20)], box(260, 300, 0, 20), bottom, 4, GAP)).toBe(36);
    });

    it('lands at the same gap for a short and a wrapped title', () => {
        expect(stackLiftTarget([box(0, 100, 0, 20)], null, bottom, 4, GAP))
            .toBe(stackLiftTarget(WRAPPED, ACTIONS_ON_FIRST_LINE, bottom, 4, GAP));
    });

    it('keeps the full stack when a wrapped meta chip occupies the target', () => {
        const chip = box(220, 300, 28, 44);
        expect(stackLiftTarget(WRAPPED, ACTIONS_ON_FIRST_LINE, bottom, 4, GAP, [chip])).toBeNull();
        const beside = box(100, 260, 28, 44);
        expect(stackLiftTarget(WRAPPED, ACTIONS_ON_FIRST_LINE, bottom, 4, GAP, [beside])).toBe(36);
    });

    it('allows a chip on the lower mark’s own meta line', () => {
        const chip = box(220, 274, 48, 64);
        expect(stackLiftTarget(WRAPPED, ACTIONS_ON_FIRST_LINE, bottom, 4, GAP, [chip])).toBe(36);
    });

    it('scales the mark, stack gap and clearance together', () => {
        const scale = (r: Box): Box => box(r.left * 2, r.right * 2, r.top * 2, r.bottom * 2);
        expect(stackLiftTarget(WRAPPED.map(scale), scale(ACTIONS_ON_FIRST_LINE), scale(bottom), 8, GAP * 2)).toBe(72);
    });
});
