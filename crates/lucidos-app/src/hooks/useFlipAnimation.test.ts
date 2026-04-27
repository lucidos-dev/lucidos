import { describe, it, expect } from 'vitest';

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
            { name: 'history', ids: ['__section_history', 'a', 'b', 'c'] },
        ]);
        const { transitioned, newItems } = detectChanges(prev, curr);
        expect(transitioned.size).toBe(0);
        expect(newItems.size).toBe(0);
    });

    it('detects no changes when sections stay the same', () => {
        const { map: prev } = buildSectionMap([
            { name: 'history', ids: ['__section_history', 'a', 'b', 'c'] },
        ]);
        const { map: curr } = buildSectionMap([
            { name: 'history', ids: ['__section_history', 'a', 'b', 'c'] },
        ]);
        const { transitioned, newItems } = detectChanges(prev, curr);
        expect(transitioned.size).toBe(0);
        expect(newItems.size).toBe(0);
    });

    it('detects thread moving from history to pinned', () => {
        const { map: prev } = buildSectionMap([
            { name: 'history', ids: ['__section_history', 'a', 'b', 'c'] },
        ]);
        const { map: curr } = buildSectionMap([
            { name: 'pinned', ids: ['__section_pinned', 'b'] },
            { name: 'history', ids: ['__section_history', 'a', 'c'] },
        ]);
        const { transitioned, newItems } = detectChanges(prev, curr);
        expect(transitioned).toEqual(new Set(['b']));
        // New section header and sections that appeared
        expect(newItems.has('__section_pinned')).toBe(true);
    });

    it('detects thread moving from pinned to history', () => {
        const { map: prev } = buildSectionMap([
            { name: 'pinned', ids: ['__section_pinned', 'b'] },
            { name: 'history', ids: ['__section_history', 'a', 'c'] },
        ]);
        const { map: curr } = buildSectionMap([
            { name: 'history', ids: ['__section_history', 'b', 'a', 'c'] },
        ]);
        const { transitioned } = detectChanges(prev, curr);
        expect(transitioned).toEqual(new Set(['b']));
    });

    it('detects new thread appearing', () => {
        const { map: prev } = buildSectionMap([
            { name: 'history', ids: ['__section_history', 'a', 'b'] },
        ]);
        const { map: curr } = buildSectionMap([
            { name: 'history', ids: ['__section_history', 'new-thread', 'a', 'b'] },
        ]);
        const { transitioned, newItems } = detectChanges(prev, curr);
        expect(transitioned.size).toBe(0);
        expect(newItems).toEqual(new Set(['new-thread']));
    });

    it('detects thread moving from running to history', () => {
        const { map: prev } = buildSectionMap([
            { name: 'running', ids: ['__section_running', 'a'] },
            { name: 'history', ids: ['__section_history', 'b', 'c'] },
        ]);
        const { map: curr } = buildSectionMap([
            { name: 'history', ids: ['__section_history', 'a', 'b', 'c'] },
        ]);
        const { transitioned, newItems } = detectChanges(prev, curr);
        expect(transitioned).toEqual(new Set(['a']));
        // __section_history was already present, __section_running disappeared
        expect(newItems.size).toBe(0);
    });

    it('detects multiple threads transitioning simultaneously', () => {
        const { map: prev } = buildSectionMap([
            { name: 'running', ids: ['__section_running', 'a', 'b'] },
            { name: 'history', ids: ['__section_history', 'c', 'd'] },
        ]);
        const { map: curr } = buildSectionMap([
            { name: 'history', ids: ['__section_history', 'a', 'b', 'c', 'd'] },
        ]);
        const { transitioned, newItems } = detectChanges(prev, curr);
        expect(transitioned).toEqual(new Set(['a', 'b']));
        expect(newItems.size).toBe(0);
    });
});

