import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here: string = dirname(fileURLToPath(import.meta.url));
const source: string = readFileSync(resolve(here, '../SettingsView.tsx'), 'utf-8');

function handler(name: string): string {
  const row = source.slice(source.indexOf('function DeviceRow('));
  const start = row.indexOf(`  function ${name}() {`);
  expect(start).toBeGreaterThan(-1);
  return row.slice(start, row.indexOf('\n  }\n', start));
}

describe('the device rename field', () => {
  it('opens with the latch reset', () => {
    expect(handler('startEditing')).toContain('closedRef.current = false');
  });

  it('saves at most once per open', () => {
    const save = handler('saveEdit');
    expect(save).toContain('closedRef.current) return');
    expect(save).toContain('closedRef.current = true');
  });

  it('cancels without letting the trailing blur save', () => {
    expect(handler('cancelEdit')).toContain('closedRef.current = true');
  });
});
