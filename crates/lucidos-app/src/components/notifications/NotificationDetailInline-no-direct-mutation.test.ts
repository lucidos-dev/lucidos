// Contract: NotificationDetailInline must not write to its store signals
// directly — every mutation goes through actions/notifications.ts (prev/next,
// close) or the navigation actions (open app / thread / nav-tap). Source-level
// regex check so a new inline `signal.value = ...` write trips the suite.
import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';

const here: string = dirname(fileURLToPath(import.meta.url));
const componentSource = readFileSync(
  resolve(here, 'NotificationDetailInline.tsx'),
  'utf-8',
);

/** Store signals NotificationDetailInline MUST NOT mutate directly.
 *  `panelOverlay` is the overlay that holds the open detail — opening / walking /
 *  closing it is owned by actions/notifications.ts. `notifications` / `unreadCount`
 *  are the inbox list + badge, also owned there. */
const FORBIDDEN_SIGNALS = [
  'panelOverlay',
  'notifications',
  'unreadCount',
] as const;

describe('NotificationDetailInline source-level contract', () => {
  for (const name of FORBIDDEN_SIGNALS) {
    it(`does not assign to ${name}.value directly`, () => {
      // Allow READ access (`name.value` on the right-hand side); reject WRITE
      // (`name.value =` or `name.value +=`/`-=`). Match `<name>.value` followed
      // by optional whitespace and an assignment operator.
      const writePattern = new RegExp(`\\b${name}\\.value\\s*[+\\-]?=[^=]`, 'g');
      const matches = componentSource.match(writePattern) ?? [];
      expect(
        matches,
        `NotificationDetailInline.tsx writes to ${name}.value directly:\n${matches.join(
          '\n',
        )}\nRoute the write through actions/notifications.ts instead.`,
      ).toEqual([]);
    });
  }
});
