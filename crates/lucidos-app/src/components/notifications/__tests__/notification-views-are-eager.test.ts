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
    expect(CONTENT_PANE).not.toMatch(new RegExp(`const ${name}\\s*=\\s*lazyComponent`));
  });

  it('still code-splits the views the rule does not cover', () => {
    expect(CONTENT_PANE).toMatch(/const SettingsView\s*=\s*lazyComponent/);
  });
});
