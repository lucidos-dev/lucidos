/**
 * The directory picker is a centered modal, and it has to take `<Overlay>`'s
 * own `.modal-overlay` container to be one.
 *
 * `.dir-picker` styles a box and nothing else: no `position`, no `z-index`, no
 * centring. So `backdrop={false}` leaves the panel laid out in flow, wherever
 * the Add Repository form sits. The rest of the page goes inert around it. It
 * shipped that way once, inside a hand-written `.confirm-overlay` wrapper whose
 * rule had already been deleted when the modal scrims were unified.
 *
 * Two halves, and both are needed. The first pins the call site. The second
 * pins the reason: give `.dir-picker` a `position` of its own and the first
 * assertion stops being a bug guard.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here: string = dirname(fileURLToPath(import.meta.url));
const source: string = readFileSync(resolve(here, '../DirectoryPicker.tsx'), 'utf-8');
const css: string = readFileSync(
  resolve(here, '../../../styles/settings/directory-picker.css'),
  'utf-8',
);

/** The component's MARKUP, with its comment lines dropped. The comment at the
 *  call site names both shapes this scans for, so a whole-file scan would fail
 *  on the note explaining the fix. */
const markup: string = source
  .split('\n')
  .filter((line: string) => {
    const trimmed = line.trim();
    return !trimmed.startsWith('//') && !trimmed.startsWith('*') && !trimmed.startsWith('/*');
  })
  .join('\n');

/** The body of the `.dir-picker` rule, so the scan cannot be satisfied by a
 *  `position` on one of its descendants. */
function dirPickerRule(): string {
  const match = /\.dir-picker\s*\{([^}]*)\}/.exec(css);
  if (!match) throw new Error('.dir-picker has no rule: was it renamed?');
  return match[1];
}

describe('DirectoryPicker is a modal', () => {
  it('renders its panel through <Overlay> with the shared backdrop container', () => {
    expect(markup).toMatch(/<Overlay\b[\s\S]{0,200}?panelClass="dir-picker"/);
    expect(markup).not.toMatch(/backdrop=\{false\}/);
  });

  it('carries no `.confirm-overlay` wrapper, whose rule is long gone', () => {
    expect(markup).not.toContain('confirm-overlay');
  });

  it('leaves the panel unplaced, so the container is what centres it', () => {
    expect(dirPickerRule()).not.toMatch(/position\s*:\s*(fixed|absolute)/);
  });
});
