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

