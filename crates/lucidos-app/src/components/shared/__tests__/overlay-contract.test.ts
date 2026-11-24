import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source: string = readFileSync(resolve(here, '../Overlay.tsx'), 'utf-8');
const SRC_ROOT: string = resolve(here, '../../..');

describe('Overlay — centralizes the dismiss contract', () => {
  it('delegates outside-click dismiss + swallow to useDismissOnOutside, forwarding the anchor', () => {
    expect(source).toMatch(/useDismissOnOutside\(\s*open\s*,\s*panelRef\s*,\s*anchor\s*,\s*onClose\s*[,)]/);
  });

  it('gates the outside-click dismiss on being the top panel of the overlay stack', () => {
    expect(source).toMatch(/useDismissOnOutside\([\s\S]{0,160}?topPanelOverlay\(\)\?\.id === idRef\.current/);
    expect(source).not.toMatch(/\btopOverlay\(\)/);
  });

  it('registers into the stack pre-paint, alongside the dismiss listeners', () => {
    expect(source).toMatch(/useLayoutEffect\(\(\) => \{[\s\S]{0,200}?pushOverlay\(/);
  });

  it('marks its stack entry as owning a panel', () => {
    expect(source).toMatch(/pushOverlay\([\s\S]{0,120}?hasPanel: true/);
  });

  it('routes Escape through the central overlay stack, not a per-instance keydown listener', () => {
    expect(source).toMatch(/pushOverlay\(/);
    expect(source).toMatch(/removeOverlay\(/);
  });

  it('derives the backdrop container z-index from its overlay-stack depth', () => {
    expect(source).toMatch(/overlayStackDepth\(\s*overlayStack\.value\s*,\s*idRef\.current\s*\)/);
    expect(source).toMatch(/z-index:\s*calc\(var\(--z-modal\)/);
  });

  it('never hand-rolls its own document dismiss listener', () => {
    expect(source).not.toMatch(/addEventListener\(\s*['"](?:pointerdown|mousedown|click)['"]/);
  });
});

describe('every onClose keeps its no-op signal to itself', () => {
  const overlayUsers: string[] = readdirSync(SRC_ROOT, { recursive: true, encoding: 'utf-8' })
    .filter((f: string) => f.endsWith('.tsx') || f.endsWith('.ts'))
    .map((f: string) => resolve(SRC_ROOT, f));

  const ASSIGNMENT_ARROW = /\(\s*\)\s*=>\s*\(?\s*[A-Za-z_$][\w$.]*\s*=\s*(?:false|null|undefined|0|''|"")\s*\)?/;

  it('never gives a dismiss callback an expression body that yields the assigned value', () => {
    const offenders: string[] = [];
    for (const file of overlayUsers) {
      const text: string = readFileSync(file, 'utf-8');
      for (const line of text.split('\n')) {
        const trimmed: string = line.trim();
        if (trimmed.startsWith('*') || trimmed.startsWith('//') || trimmed.startsWith('/*')) continue;
        if (!/onClose|onDismiss|\bdismiss\b|close/i.test(line)) continue;
        if (ASSIGNMENT_ARROW.test(line)) {
          offenders.push(`${file.slice(SRC_ROOT.length + 1)}: ${line.trim()}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });
});

describe('Dropdown hands the Overlay a close that returns nothing', () => {
  it('declares closeDropdown void', () => {
    const dropdown: string = readFileSync(resolve(here, '../Dropdown.tsx'), 'utf-8');
    expect(dropdown).toContain('onClose={closeDropdown}');
    expect(dropdown).toMatch(/function closeDropdown\(\): void \{/);
  });
});
