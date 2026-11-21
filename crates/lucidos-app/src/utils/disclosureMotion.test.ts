import { describe, it, expect } from 'vitest';
import {
    disclosureDurationMs, rollCap, DISCLOSURE_MIN_MS, DISCLOSURE_MAX_MS,
} from './disclosureMotion';

describe('disclosureDurationMs', () => {
    it('grows with the block and stays within its bounds', () => {
        expect(disclosureDurationMs(10)).toBe(DISCLOSURE_MIN_MS);
        expect(disclosureDurationMs(5000)).toBe(DISCLOSURE_MAX_MS);
        expect(disclosureDurationMs(400)).toBeGreaterThan(disclosureDurationMs(350));
    });

    it('rolls a phone screen of rows well inside half a second', () => {
        expect(disclosureDurationMs(700)).toBeLessThan(500);
    });
});

describe('rollCap', () => {
    it('rolls a block that fits on screen its whole height', () => {
        expect(rollCap(120, 100, 800)).toBe(120);
    });

    it('rolls a long block only down to the bottom edge', () => {
        expect(rollCap(3000, 200, 800)).toBe(600);
    });

    it('does not roll a block whose line is below the screen', () => {
        expect(rollCap(120, 900, 800)).toBe(0);
    });
});
