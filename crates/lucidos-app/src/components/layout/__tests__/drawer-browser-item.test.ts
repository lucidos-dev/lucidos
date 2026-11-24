// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const drawerSource = readFileSync(
  fileURLToPath(new URL('../Drawer.tsx', import.meta.url)),
  'utf8',
);

function gatesBefore(index: number): string[] {
  return [...drawerSource.slice(0, index).matchAll(/\{([^{}\n]+?) && \(/g)].map((m) => m[1]);
}

describe('the menu drawer Browser row', () => {
  it('is gated on inAppBrowserAvailable(), never on the platform check alone', () => {
    const label = drawerSource.match(/^[ \t]*Browser$/m);
    expect(label?.index, 'the Browser row label').toBeDefined();

    const gates = gatesBefore(label!.index!);
    expect(gates[gates.length - 1]).toBe('inAppBrowserAvailable()');
  });
});
