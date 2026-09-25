// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
    blockExtent, contiguousRuns, crossesViewport, disclosureDurationMs, ghostOf, mountDepartureLayer, mountDisclosureMask,
    rollDistance, rowEndScale, startTogetherOnNextFrame, tuckTarget,
} from './useFlipAnimation';

describe('contiguousRuns', () => {
    it('groups adjacent members under the row before them', () => {
        const order = ['__section_saved', 'p', 'c1', 'c2', 'q', 'd1'];
        expect(contiguousRuns(order, new Set(['c1', 'c2', 'd1']))).toEqual([
            { anchor: 'p', ids: ['c1', 'c2'] },
            { anchor: 'q', ids: ['d1'] },
        ]);
    });

    it('has no anchor for a run at the top of the list', () => {
        expect(contiguousRuns(['a', 'b', 'c'], new Set(['a', 'b']))).toEqual([
            { anchor: null, ids: ['a', 'b'] },
        ]);
    });

    it('returns nothing when no row is a member', () => {
        expect(contiguousRuns(['a', 'b'], new Set())).toEqual([]);
    });
});

describe('mountDisclosureMask', () => {
    // jsdom lays nothing out, so the mask's own origin reads as (0, 0).
    const rect = (top: number, height = 40) => ({ top, left: 16, width: 300, height });

    it('starts the mask at the reveal line and sizes it to the rows below it', () => {
        const host = document.createElement('div');
        const ghosts = [document.createElement('div'), document.createElement('div')];
        const mask = mountDisclosureMask(host, 100, [
            { ghost: ghosts[0], rect: rect(100) },
            { ghost: ghosts[1], rect: rect(140) },
        ]);
        expect(mask.parentElement).toBe(host);
        expect(mask.className).toBe('flip-disclosure-mask');
        expect(mask.style.top).toBe('100px');
        expect(mask.style.height).toBe('80px');
    });

    it('places each copy relative to the line, at its own box', () => {
        const host = document.createElement('div');
        const ghost = document.createElement('div');
        const mask = mountDisclosureMask(host, 100, [{ ghost, rect: rect(140) }]);
        expect(ghost.parentElement).toBe(mask);
        expect(ghost.style.position).toBe('absolute');
        expect(ghost.style.top).toBe('40px');
        expect(ghost.style.left).toBe('16px');
        expect(ghost.style.width).toBe('300px');
        expect(ghost.style.height).toBe('40px');
    });

    it('stays out of the accessibility tree and the tab order', () => {
        const mask = mountDisclosureMask(document.createElement('div'), 0, [
            { ghost: document.createElement('div'), rect: rect(0) },
        ]);
        expect(mask.getAttribute('aria-hidden')).toBe('true');
        expect(mask.inert).toBe(true);
    });
});

describe('blockExtent', () => {
    const rect = (top: number, height = 40) => ({ top, left: 0, width: 300, height });

    it('runs from the anchor\'s bottom edge to the lowest row\'s bottom edge', () => {
        expect(blockExtent([rect(100), rect(140)], rect(60))).toEqual({ line: 100, travel: 80 });
    });

    it('starts at the first row when there is no anchor', () => {
        expect(blockExtent([rect(0), rect(40)], undefined)).toEqual({ line: 0, travel: 80 });
    });
});

describe('crossesViewport', () => {
    const rect = (top: number, height = 40) => ({ top, left: 0, width: 300, height });

    it('copies a row that starts below the fold but slides through the screen', () => {
        // A 40-row block under a header at 100px, in an 800px viewport.
        expect(crossesViewport(rect(1500), 100, 1600, 800)).toBe(true);
    });

    it('skips a row whose whole slide stays below the fold', () => {
        expect(crossesViewport(rect(3000), 100, 1600, 800)).toBe(false);
    });

    it('skips a row scrolled above the viewport', () => {
        expect(crossesViewport(rect(-500), -600, 1600, 800)).toBe(false);
    });
});

describe('rollDistance', () => {
    const rows = (count: number, line: number, height = 50) =>
        Array.from({ length: count }, (_, i) => ({ top: line + i * height, left: 0, width: 300, height }));

    it('rolls a block that fits on screen its whole height', () => {
        expect(rollDistance(rows(4, 100), 100, 200, 800)).toBe(200);
    });

    it('cuts a long block after the last row that starts on screen', () => {
        // The last row to start above 800 starts at 750. Its bottom edge,
        // 800, is 700 below the line.
        expect(rollDistance(rows(40, 100), 100, 2000, 800)).toBe(700);
    });

    it('does not roll once the line has scrolled above the screen', () => {
        // No parent or header is on screen to roll under, so it lands at once.
        expect(rollDistance(rows(80, -2000), -2000, 4000, 800)).toBe(0);
    });

    it('does not roll a block that starts below the fold', () => {
        expect(rollDistance(rows(3, 900), 900, 150, 800)).toBe(0);
    });
});

describe('rowEndScale', () => {
    const line = { left: 16, width: 200 };

    it('keeps the full width when the closing row is top-level', () => {
        expect(rowEndScale(line, 16)).toBe(1);
    });

    it('narrows to a nested row\'s indent, sharing the right edge', () => {
        // The row's line starts 20px further in, so it is 180 of the 200px.
        expect(rowEndScale(line, 36)).toBe(0.9);
    });

    it('stays full width when the row draws no line or the line has no width', () => {
        expect(rowEndScale(line, null)).toBe(1);
        expect(rowEndScale({ left: 16, width: 0 }, 36)).toBe(1);
    });
});

describe('disclosureDurationMs', () => {
    it('grows with the block and stays within its bounds', () => {
        expect(disclosureDurationMs(10)).toBe(260);
        expect(disclosureDurationMs(5000)).toBe(420);
        expect(disclosureDurationMs(400)).toBeGreaterThan(disclosureDurationMs(350));
    });

    it('rolls a phone screen of rows well inside half a second', () => {
        expect(disclosureDurationMs(700)).toBeLessThanOrEqual(420);
    });
});

describe('startTogetherOnNextFrame', () => {
    const fakeAnim = () => ({ pause: vi.fn(), play: vi.fn(), startTime: null as number | null });
    let frames: FrameRequestCallback[] = [];
    beforeEach(() => {
        frames = [];
        vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => { frames.push(cb); return frames.length; });
    });
    afterEach(() => vi.unstubAllGlobals());

    it('holds every animation until the next frame, then starts them together', () => {
        const anims = [fakeAnim(), fakeAnim()];
        startTogetherOnNextFrame(anims as unknown as Animation[], () => true);
        for (const a of anims) expect(a.pause).toHaveBeenCalled();
        expect(anims.every(a => a.play.mock.calls.length === 0 && a.startTime === null)).toBe(true);
        frames[0](0);
        for (const a of anims) expect(a.play.mock.calls.length > 0 || a.startTime !== null).toBe(true);
    });

    it('leaves a batch a newer one has taken over', () => {
        const anim = fakeAnim();
        startTogetherOnNextFrame([anim] as unknown as Animation[], () => false);
        frames[0](0);
        expect(anim.play).not.toHaveBeenCalled();
        expect(anim.startTime).toBeNull();
    });
});

describe('tuckTarget', () => {
    const sections = [
        { name: 'current', ids: ['__section_current', 'a'] },
        { name: 'archive', ids: ['__section_archive'], tucked: ['b', 'c'] },
    ];

    it('folds a thread hidden in a collapsed section into that section\'s header', () => {
        expect(tuckTarget('b', sections)).toBe('__section_archive');
    });

    it('has no target for a thread that left every section', () => {
        expect(tuckTarget('gone', sections)).toBeNull();
    });
});

describe('mountDepartureLayer', () => {
    const rect = (top: number) => ({ top, left: 16, width: 300, height: 40 });

    it('places each copy at its own box, inert and hidden from assistive tech', () => {
        const host = document.createElement('div');
        const ghost = document.createElement('div');
        const layer = mountDepartureLayer(host, [{ ghost, rect: rect(140) }]);
        expect(layer.parentElement).toBe(host);
        expect(layer.className).toBe('flip-departure-layer');
        expect(layer.getAttribute('aria-hidden')).toBe('true');
        expect(layer.inert).toBe(true);
        expect(ghost.parentElement).toBe(layer);
        expect(ghost.style.position).toBe('absolute');
        expect(ghost.style.top).toBe('140px');
        expect(ghost.style.left).toBe('16px');
        expect(ghost.style.width).toBe('300px');
    });
});

describe('ghostOf', () => {
    it('drops every attribute a lookup could find the real row by', () => {
        const row = document.createElement('div');
        row.dataset.flipId = 'c1';
        row.innerHTML = '<div id="nav-c1" class="thread-row" data-thread-nav="c1"><button>Hide</button></div>';
        const ghost = ghostOf(row);
        expect(ghost.hasAttribute('data-flip-id')).toBe(false);
        expect(ghost.querySelector('[id], [data-thread-nav], [data-flip-id]')).toBeNull();
        expect(ghost.getAttribute('aria-hidden')).toBe('true');
        expect(ghost.inert).toBe(true);
        // The real row keeps its own attributes.
        expect(row.querySelector('[data-thread-nav="c1"]')).not.toBeNull();
    });
});

// Extract the pure logic from useFlipTransitions for testing.
// These functions mirror the inline logic in the hook.

function buildSectionMap(sections: { name: string; ids: string[] }[]) {
    const map = new Map<string, string>();
    for (const section of sections) {
        for (const id of section.ids) {
            map.set(id, section.name);
        }
    }
    return { map };
}

function detectChanges(
    prevSections: Map<string, string>,
    currentSections: Map<string, string>,
) {
    const transitioned = new Set<string>();
    const newItems = new Set<string>();
    if (prevSections.size > 0) {
        for (const [id, section] of currentSections) {
            const prevSection = prevSections.get(id);
            if (!prevSection) {
                newItems.add(id);
            } else if (prevSection !== section) {
                transitioned.add(id);
            }
        }
    }
    return { transitioned, newItems };
}

describe('FLIP transition detection', () => {
    it('detects no changes on first render', () => {
        const prev = new Map<string, string>();
        const { map: curr } = buildSectionMap([
            { name: 'archive', ids: ['__section_archive', 'a', 'b', 'c'] },
        ]);
        const { transitioned, newItems } = detectChanges(prev, curr);
        expect(transitioned.size).toBe(0);
        expect(newItems.size).toBe(0);
    });

    it('detects no changes when sections stay the same', () => {
        const { map: prev } = buildSectionMap([
            { name: 'archive', ids: ['__section_archive', 'a', 'b', 'c'] },
        ]);
        const { map: curr } = buildSectionMap([
            { name: 'archive', ids: ['__section_archive', 'a', 'b', 'c'] },
        ]);
        const { transitioned, newItems } = detectChanges(prev, curr);
        expect(transitioned.size).toBe(0);
        expect(newItems.size).toBe(0);
    });

    it('detects thread moving from archive to saved', () => {
        const { map: prev } = buildSectionMap([
            { name: 'archive', ids: ['__section_archive', 'a', 'b', 'c'] },
        ]);
        const { map: curr } = buildSectionMap([
            { name: 'saved', ids: ['__section_saved', 'b'] },
            { name: 'archive', ids: ['__section_archive', 'a', 'c'] },
        ]);
        const { transitioned, newItems } = detectChanges(prev, curr);
        expect(transitioned).toEqual(new Set(['b']));
        // New section header and sections that appeared
        expect(newItems.has('__section_saved')).toBe(true);
    });

    it('detects thread moving from saved to archive', () => {
        const { map: prev } = buildSectionMap([
            { name: 'saved', ids: ['__section_saved', 'b'] },
            { name: 'archive', ids: ['__section_archive', 'a', 'c'] },
        ]);
        const { map: curr } = buildSectionMap([
            { name: 'archive', ids: ['__section_archive', 'b', 'a', 'c'] },
        ]);
        const { transitioned } = detectChanges(prev, curr);
        expect(transitioned).toEqual(new Set(['b']));
    });

    it('detects new thread appearing', () => {
        const { map: prev } = buildSectionMap([
            { name: 'archive', ids: ['__section_archive', 'a', 'b'] },
        ]);
        const { map: curr } = buildSectionMap([
            { name: 'archive', ids: ['__section_archive', 'new-thread', 'a', 'b'] },
        ]);
        const { transitioned, newItems } = detectChanges(prev, curr);
        expect(transitioned.size).toBe(0);
        expect(newItems).toEqual(new Set(['new-thread']));
    });

    it('detects thread moving from active to archive', () => {
        const { map: prev } = buildSectionMap([
            { name: 'active', ids: ['__section_active', 'a'] },
            { name: 'archive', ids: ['__section_archive', 'b', 'c'] },
        ]);
        const { map: curr } = buildSectionMap([
            { name: 'archive', ids: ['__section_archive', 'a', 'b', 'c'] },
        ]);
        const { transitioned, newItems } = detectChanges(prev, curr);
        expect(transitioned).toEqual(new Set(['a']));
        // __section_archive was already present, __section_active disappeared
        expect(newItems.size).toBe(0);
    });

    it('detects multiple threads transitioning simultaneously', () => {
        const { map: prev } = buildSectionMap([
            { name: 'active', ids: ['__section_active', 'a', 'b'] },
            { name: 'archive', ids: ['__section_archive', 'c', 'd'] },
        ]);
        const { map: curr } = buildSectionMap([
            { name: 'archive', ids: ['__section_archive', 'a', 'b', 'c', 'd'] },
        ]);
        const { transitioned, newItems } = detectChanges(prev, curr);
        expect(transitioned).toEqual(new Set(['a', 'b']));
        expect(newItems.size).toBe(0);
    });
});

