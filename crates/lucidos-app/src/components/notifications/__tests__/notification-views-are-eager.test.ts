/**
 * The two notification views must not go back behind `lazyComponent`.
 *
 * `lazyComponent` renders `null` until its chunk arrives, so a lazy view paints
 * NOTHING in the meantime: no toolbar, no skeleton, no empty state. Every other
 * content view can afford that, because you reach it from the menu with the pane
 * already in front of you. You reach these two from the bell in every header,
 * and from an OS push tap. So the chunk fetch lands in front of the first pixel
 * of a surface the user opened from outside the app.
 *
 * That is the reported bug: opening Notifications drew a blank panel, with the
 * All / Unread toggle and the row skeleton both stuck inside the chunk. The
 * detail carries the same hole, and its skeleton is what the chunk swallows.
 *
 * A source scan, for the reason `startup.test.ts` gives: standing the pane up
 * in jsdom to observe a chunk that never loads would pin the mechanism rather
 * than the requirement.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here: string = dirname(fileURLToPath(import.meta.url));
const CONTENT_PANE: string = readFileSync(resolve(here, '../../layout/ContentPane.tsx'), 'utf-8');

const EAGER = [
  ['NotificationsView', '../notifications/NotificationsView'],
  ['NotificationDetailInline', '../notifications/NotificationDetailInline'],
] as const;

describe('ContentPane keeps the notification views out of the code-split', () => {
  it.each(EAGER)('imports %s statically', (name, path) => {
    expect(CONTENT_PANE).toContain(`import { ${name} } from '${path}'`);
  });

  it.each(EAGER)('never wraps %s in lazyComponent', (name) => {
    // Matches the declaration form the other views use, so the assertion reads
    // the same way a reviewer would scan the block.
    expect(CONTENT_PANE).not.toMatch(new RegExp(`const ${name}\\s*=\\s*lazyComponent`));
  });

  it('still code-splits the views the rule does not cover', () => {
    // Proves the scan can see a lazy declaration at all. Without this the two
    // assertions above would pass on an empty file.
    expect(CONTENT_PANE).toMatch(/const SettingsView\s*=\s*lazyComponent/);
  });
});
