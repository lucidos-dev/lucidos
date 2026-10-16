import { describe, it, expect } from 'vitest';
import { planRowWindow, RowHeights, needsNewSpan, renderSpanFor, segmentsKey, type RowSegment } from './rowWindow';

/** The heights a plan renders and skips, flattened: every row index in
 *  order, rendered or folded into a gap. */
function renderedIndices(segments: RowSegment[]): number[] {
    return segments.flatMap(s => s.kind === 'rows'
        ? Array.from({ length: s.to - s.from }, (_, i) => s.from + i)
        : []);
}

function gapHeights(segments: RowSegment[]): number[] {
    return segments.filter(s => s.kind === 'gap').map(s => s.height);
}

describe('planRowWindow', () => {
    const fixed = (h: number) => () => h;

    it('renders only the rows that overlap a span', () => {
        // 100 rows of 10px; the span covers 200..300, so rows 20..29.
        const segments = planRowWindow({
            count: 100, heightAt: fixed(10), estimate: 10,
            spans: [{ top: 200, bottom: 300 }], pinned: new Set(),
        });
        expect(renderedIndices(segments)).toEqual([20, 21, 22, 23, 24, 25, 26, 27, 28, 29]);
        // One gap above and one below, each the exact sum of its rows.
        expect(gapHeights(segments)).toEqual([200, 700]);
    });

    it('sums measured heights exactly into the gaps', () => {
        const heights = [10.5, 20.25, 30.125, 5, 5, 5];
        const segments = planRowWindow({
            count: heights.length, heightAt: i => heights[i], estimate: 99,
            spans: [{ top: 66, bottom: 67 }], pinned: new Set(),
        });
        // Rows 0..2 end at 60.875, row 3 at 65.875, row 4 covers 66.
        expect(renderedIndices(segments)).toEqual([4]);
        expect(gapHeights(segments)).toEqual([10.5 + 20.25 + 30.125 + 5, 5]);
    });

    it('renders every unknown row above a span instead of estimating it', () => {
        // Rows 0..49 have never been measured. The span sits at row 80.
        const segments = planRowWindow({
            count: 100, heightAt: i => (i < 50 ? undefined : 10), estimate: 10,
            spans: [{ top: 800, bottom: 810 }], pinned: new Set(),
        });
        const rendered = renderedIndices(segments);
        for (let i = 0; i < 50; i++) expect(rendered).toContain(i);
        expect(rendered).toContain(80);
        // The gap between them holds only measured rows, never an estimate.
        expect(gapHeights(segments)[0]).toBe(300);
    });

    it('lets unknown rows below every span stay estimated', () => {
        const segments = planRowWindow({
            count: 100, heightAt: i => (i < 10 ? 10 : undefined), estimate: 7,
            spans: [{ top: 0, bottom: 50 }], pinned: new Set(),
        });
        expect(renderedIndices(segments)).toEqual([0, 1, 2, 3, 4]);
        // Rows 5..9 are measured, 10..99 are estimated.
        expect(gapHeights(segments)).toEqual([5 * 10 + 90 * 7]);
    });

    it('treats a section wholly above the spans as above: unknown rows render', () => {
        // The viewport is below the end of this section.
        const segments = planRowWindow({
            count: 5, heightAt: i => (i === 2 ? undefined : 10), estimate: 10,
            spans: [{ top: 500, bottom: 600 }], pinned: new Set(),
        });
        expect(renderedIndices(segments)).toEqual([2]);
        expect(gapHeights(segments)).toEqual([20, 20]);
    });

    it('renders unknown rows between two spans, since they sit above the lower one', () => {
        const segments = planRowWindow({
            count: 100, heightAt: i => (i >= 40 && i < 45 ? undefined : 10), estimate: 10,
            spans: [{ top: 0, bottom: 100 }, { top: 700, bottom: 800 }], pinned: new Set(),
        });
        const rendered = renderedIndices(segments);
        for (let i = 40; i < 45; i++) expect(rendered).toContain(i);
    });

    it('renders a pinned row wherever it sits, as a one-row segment', () => {
        const segments = planRowWindow({
            count: 100, heightAt: fixed(10), estimate: 10,
            spans: [{ top: 0, bottom: 30 }], pinned: new Set([70]),
        });
        expect(renderedIndices(segments)).toEqual([0, 1, 2, 70]);
        expect(gapHeights(segments)).toEqual([670, 290]);
    });

    it('ignores a pin outside the list', () => {
        const segments = planRowWindow({
            count: 3, heightAt: fixed(10), estimate: 10,
            spans: [{ top: 0, bottom: 10 }], pinned: new Set([-1, 9]),
        });
        expect(renderedIndices(segments)).toEqual([0]);
    });

    it('plans nothing for an empty section', () => {
        expect(planRowWindow({ count: 0, heightAt: fixed(10), estimate: 10, spans: [], pinned: new Set() })).toEqual([]);
    });

    it('keys equal plans equally and different plans differently', () => {
        const input = { count: 50, heightAt: fixed(10), estimate: 10, pinned: new Set<number>() };
        const a = planRowWindow({ ...input, spans: [{ top: 100, bottom: 200 }] });
        const b = planRowWindow({ ...input, spans: [{ top: 101, bottom: 199 }] });
        const c = planRowWindow({ ...input, spans: [{ top: 300, bottom: 400 }] });
        expect(segmentsKey(a)).toBe(segmentsKey(b));
        expect(segmentsKey(a)).not.toBe(segmentsKey(c));
    });
});

describe('RowHeights', () => {
    it('answers a height only for the layout key it was measured under', () => {
        const heights = new RowHeights();
        heights.record('a', 'k1', 40);
        expect(heights.valid('a', 'k1')).toBe(40);
        expect(heights.valid('a', 'k2')).toBeUndefined();
        expect(heights.valid('b', 'k1')).toBeUndefined();
    });

    it('drops every height when the environment changes', () => {
        const heights = new RowHeights();
        heights.record('a', 'k', 40);
        heights.invalidate();
        expect(heights.valid('a', 'k')).toBeUndefined();
        heights.record('a', 'k', 44);
        expect(heights.valid('a', 'k')).toBe(44);
    });

    it('reports an environment change when a row resizes under the same key', () => {
        const heights = new RowHeights();
        expect(heights.record('a', 'k', 40)).toBe('new');
        expect(heights.record('a', 'k', 40)).toBe('same');
        // Sub-pixel noise is not a change.
        expect(heights.record('a', 'k', 40.25)).toBe('same');
        expect(heights.record('a', 'k', 48)).toBe('environment');
        // A new key is new content, not a new environment.
        expect(heights.record('a', 'k2', 60)).toBe('new');
    });

    it('drops every height when it moves to a different environment', () => {
        const heights = new RowHeights();
        expect(heights.useEnvironment('300:16')).toBe(false);
        heights.record('a', 'k', 40);
        expect(heights.useEnvironment('300:16')).toBe(false);
        expect(heights.valid('a', 'k')).toBe(40);
        const before = heights.generation;
        expect(heights.useEnvironment('320:16')).toBe(true);
        expect(heights.generation).toBe(before + 1);
        expect(heights.valid('a', 'k')).toBeUndefined();
    });

    it('keeps its estimate current as a row is measured again', () => {
        const heights = new RowHeights();
        heights.record('a', 'k', 40);
        heights.record('a', 'k2', 60);
        expect(heights.estimate(50)).toBe(60);
    });

    it('estimates from the measured rows, or a fallback before any', () => {
        const heights = new RowHeights();
        expect(heights.estimate(50)).toBe(50);
        heights.record('a', 'k', 40);
        heights.record('b', 'k', 70);
        expect(heights.estimate(50)).toBe(55);
        heights.invalidate();
        expect(heights.estimate(33)).toBe(33);
    });
});

describe('span hysteresis', () => {
    const viewport = { top: 1000, bottom: 1500 };

    it('renders two viewports either side of the viewport', () => {
        expect(renderSpanFor(viewport)).toEqual({ top: 0, bottom: 2500 });
    });

    it('keeps a span while it still covers a viewport of margin either side', () => {
        const span = renderSpanFor(viewport);
        expect(needsNewSpan(span, viewport)).toBe(false);
        expect(needsNewSpan(span, { top: 1400, bottom: 1900 })).toBe(false);
        expect(needsNewSpan(span, { top: 1600, bottom: 2100 })).toBe(true);
        expect(needsNewSpan(span, { top: 400, bottom: 900 })).toBe(true);
        expect(needsNewSpan(null, viewport)).toBe(true);
    });
});
