/**
 * `stepThreadInList` decides where the next/previous thread shortcuts land:
 * the neighbouring thread row of the drawer's open list, staying inside the
 * focused thread's lifecycle section.
 */

import { describe, it, expect } from 'vitest';
import { stepThreadInList, type DrawerNavNode } from './ThreadDrawer';
import type { DisplaySection } from '../../generated/thread-lifecycle';

function section(sectionKey: DisplaySection): DrawerNavNode {
    return { kind: 'section', sectionKey };
}

function row(id: string, sectionKey: DisplaySection | null, depth = 0): DrawerNavNode {
    return { kind: 'thread', id, depth, parentId: null, hasChildren: false, sectionKey };
}

describe('stepThreadInList', () => {
    const flat = [row('a', null), row('b', null), row('c', null)];

    it('steps to the neighbouring row of a flat status view', () => {
        expect(stepThreadInList(flat, 'b', 1)).toBe('c');
        expect(stepThreadInList(flat, 'b', -1)).toBe('a');
    });

    it('stops at either end rather than wrapping', () => {
        expect(stepThreadInList(flat, 'c', 1)).toBeNull();
        expect(stepThreadInList(flat, 'a', -1)).toBeNull();
    });

    it('starts at the first or last row when the focused thread is not listed', () => {
        expect(stepThreadInList(flat, 'elsewhere', 1)).toBe('a');
        expect(stepThreadInList(flat, null, -1)).toBe('c');
    });

    it('stays inside the focused thread\'s section and skips headers', () => {
        const nodes = [
            section('saved'), row('p1', 'saved'), row('p2', 'saved'),
            section('current'), row('c1', 'current'), row('c1-child', 'current', 1), row('c2', 'current'),
        ];
        expect(stepThreadInList(nodes, 'p2', 1)).toBeNull();
        expect(stepThreadInList(nodes, 'c1', -1)).toBeNull();
        expect(stepThreadInList(nodes, 'c1', 1)).toBe('c1-child');
        expect(stepThreadInList(nodes, 'c1-child', 1)).toBe('c2');
    });

    it('returns null for an empty list', () => {
        expect(stepThreadInList([], 'a', 1)).toBeNull();
    });
});
