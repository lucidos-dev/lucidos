import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { expect, it } from 'vitest';

it('openSettingsView waits for the event stream before it posts the navigation', () => {
  const source = readFileSync(join(__dirname, 'helpers.ts'), 'utf8');
  const start = source.indexOf('export async function openSettingsView');
  const body = source.slice(start, source.indexOf('\n}\n', start));
  const wait = body.indexOf('waitForEventStream(page)');
  expect(wait).toBeGreaterThan(-1);
  expect(wait).toBeLessThan(body.indexOf("post('/api/v1/ui/navigate'"));
});
