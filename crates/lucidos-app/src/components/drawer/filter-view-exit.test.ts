/**
 * Every status-filter view in the thread drawer (Drafts / Needs attention /
 * Review / Running) offers the same way out: a "See all statuses" shortcut back
 * to the unfiltered view. It has to be there in BOTH states, because the user is
 * equally stuck either way. An empty filter says "nothing here", and a filter
 * with two rows says "these two, and nothing else". In both cases whatever else
 * they're looking for lives under another status, and the exit belongs at the
 * end of the list they just read rather than back up in the filter control.
 *
 * The regression this guards is a view keeping the empty-state shortcut while
 * losing (or never gaining) the trailing one, so the exit blinks out of
 * existence the moment a thread lands in the filter. The four lists are
 * hook-bearing components and the test infra has no jsdom, so this is a
 * source-scan, the same shape as `components/shared/__tests__/skeleton-guard.test.ts`.
 */

import { describe, it, expect } from 'vitest';
// @ts-expect-error Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error same
import { fileURLToPath } from 'node:url';
// @ts-expect-error same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const SOURCE = readFileSync(resolve(here, 'ThreadDrawer.tsx'), 'utf8');

/** Top-level `function Name() { … }` blocks, keyed by name. Splitting on the
 *  column-0 `function` keyword is enough here: the file declares every component
 *  that way, and a nested function is indented. */
function topLevelFunctions(src: string): Map<string, string> {
    const out = new Map<string, string>();
    for (const chunk of src.split(/^function /m).slice(1)) {
        const name = chunk.match(/^(\w+)/)?.[1];
        if (name) out.set(name, chunk);
    }
    return out;
}

describe('status-filter view exit shortcut', () => {
    it('gives every view that has an empty-state shortcut a trailing one too', () => {
        const withEmptyState: string[] = [];
        const missingFooter: string[] = [];
        for (const [name, body] of topLevelFunctions(SOURCE)) {
            if (!body.includes('<EmptyFilteredView')) continue;
            withEmptyState.push(name);
            if (!body.includes('<FilteredViewFooter')) missingFooter.push(name);
        }

        // Guard the guard: if the empty-state component is ever renamed, the scan
        // above would find nothing and vacuously pass.
        expect(withEmptyState.length).toBeGreaterThan(0);
        expect(missingFooter, 'a filtered view must offer "See all statuses" with rows too, not only when empty').toEqual([]);
    });

    it('renders both states through the one shortcut component', () => {
        const fns = topLevelFunctions(SOURCE);
        expect(fns.get('EmptyFilteredView')).toContain('<SeeAllStatusesLink');
        expect(fns.get('FilteredViewFooter')).toContain('<SeeAllStatusesLink');
        // The label and the link markup live in exactly one place, so re-inlining
        // a copy in either state (the way the two would drift apart) fails here.
        expect(fns.get('SeeAllStatusesLink')).toContain('See all statuses');
        expect(fns.get('EmptyFilteredView')).not.toContain('accent-link');
        expect(fns.get('FilteredViewFooter')).not.toContain('accent-link');
    });
});
