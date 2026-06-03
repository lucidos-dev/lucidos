// Contract: NotificationsModal must not write to its store signals directly
// — every mutation goes through actions/notifications.ts. Source-level
// regex check so a new inline `signal.value = ...` write trips the suite.
import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';

const here: string = dirname(fileURLToPath(import.meta.url));
const modalSource = readFileSync(
  resolve(here, 'NotificationsModal.tsx'),
  'utf-8',
);

/** Store signals NotificationsModal MUST NOT mutate directly. Each name is
 *  re-exported from `../../store/store` and owned by `actions/notifications.ts`. */
const FORBIDDEN_SIGNALS = [
  'notificationsModalOpen',
  'notificationModalDetail',
  'notifications',
  'unreadCount',
] as const;

describe('NotificationsModal source-level contract', () => {
  for (const name of FORBIDDEN_SIGNALS) {
    it(`does not assign to ${name}.value directly`, () => {
      // Allow READ access (`name.value` on the right-hand side); reject WRITE
      // (`name.value =` or `name.value +=`/`-=`). Match `<name>.value` followed
      // by optional whitespace and an assignment operator.
      const writePattern = new RegExp(`\\b${name}\\.value\\s*[+\\-]?=[^=]`, 'g');
      const matches = modalSource.match(writePattern) ?? [];
      expect(
        matches,
        `NotificationsModal.tsx writes to ${name}.value directly:\n${matches.join(
          '\n',
        )}\nRoute the write through actions/notifications.ts instead.`,
      ).toEqual([]);
    });
  }
});
