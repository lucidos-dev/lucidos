/**
 * Tests for `setArchivedRevealed` / `toggleArchivedRevealed` — the action
 * behind a row's "N archived" reveal toggle. Mirrors
 * `focused-thread-family-toggle.test.ts`: per-thread state is
 * localStorage-backed, mirroring `collapsedFamilies`, so we assert via the
 * persisted set rather than a private signal.
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { setArchivedRevealed, toggleArchivedRevealed } from './ThreadDrawer';

const REVEALED_ARCHIVED_KEY = 'lucidos-drawer-revealed-archived';

function persistedRevealed(): string[] {
    const raw = localStorage.getItem(REVEALED_ARCHIVED_KEY);
    return raw ? (JSON.parse(raw) as string[]) : [];
}

describe('setArchivedRevealed / toggleArchivedRevealed', () => {
    beforeEach(() => {
        localStorage.clear();
    });

    it('reveals then hides a thread by id', () => {
        setArchivedRevealed('parent-1', true);
        expect(persistedRevealed()).toContain('parent-1');

        setArchivedRevealed('parent-1', false);
        expect(persistedRevealed()).not.toContain('parent-1');
    });

    it('toggles from the current state', () => {
        toggleArchivedRevealed('parent-2');
        expect(persistedRevealed()).toContain('parent-2');

        toggleArchivedRevealed('parent-2');
        expect(persistedRevealed()).not.toContain('parent-2');
    });

    it('tracks each thread independently', () => {
        setArchivedRevealed('a', true);
        setArchivedRevealed('b', true);
        setArchivedRevealed('a', false);

        expect(persistedRevealed().sort()).toEqual(['b']);
    });

    it('is a no-op when already in the requested state', () => {
        setArchivedRevealed('c', true);
        const before = localStorage.getItem(REVEALED_ARCHIVED_KEY);
        setArchivedRevealed('c', true);
        expect(localStorage.getItem(REVEALED_ARCHIVED_KEY)).toBe(before);
    });
});
