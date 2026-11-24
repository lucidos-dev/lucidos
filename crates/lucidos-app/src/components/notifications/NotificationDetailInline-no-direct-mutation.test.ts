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

const FORBIDDEN_SIGNALS = [
  'panelOverlay',
  'notifications',
  'unreadCount',
] as const;

describe('NotificationDetailInline source-level contract', () => {
  for (const name of FORBIDDEN_SIGNALS) {
    it(`does not assign to ${name}.value directly`, () => {
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
