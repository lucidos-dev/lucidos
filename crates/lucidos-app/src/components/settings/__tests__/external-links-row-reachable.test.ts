/**
 * The External links row must live under a settings nav entry the target
 * platform can actually SEE.
 *
 * The bug this pins: the row's own visibility predicate
 * (`externalLinkTargetConfigurable`, iOS-PWA-only) was correct and unit-tested,
 * and the row was still unreachable, because it was rendered inside the
 * **Experimental** subview whose nav entry is filtered to `isTauri()`. Nothing
 * failed. On a desktop browser the predicate hid the row; on an installed iOS
 * PWA the nav entry hid the whole subview. The setting existed and no user could
 * open it.
 *
 * A predicate test cannot catch that: it asks "would this row render?", never
 * "can anyone navigate to where it renders?". So this guard checks placement,
 * which is the property that actually broke.
 *
 * Source-scan rather than a mounted render: `SettingsView` pulls in the whole
 * store, the model registry, OAuth and device state, so standing it up to
 * observe one section's position would pin the mechanism instead of the
 * requirement (the same reasoning as `useStartup.test.ts`).
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const SOURCE = readFileSync(resolve(here, '..', 'SettingsView.tsx'), 'utf8');

/** Strip comments so the prose explaining a call can never stand in for it. */
function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^\\:])\/\/.*$/gm, '$1');
}

/** The body of a `function <name>()` declaration, by brace matching. */
function functionBody(src: string, declaration: string): string {
  const stripped = stripComments(src);
  const start = stripped.indexOf(declaration);
  expect(start, `SettingsView.tsx must declare \`${declaration}\``).toBeGreaterThan(-1);
  const open = stripped.indexOf('{', start);
  let depth = 0;
  for (let i = open; i < stripped.length; i++) {
    if (stripped[i] === '{') depth++;
    else if (stripped[i] === '}' && --depth === 0) return stripped.slice(open + 1, i);
  }
  throw new Error(`unbalanced braces in \`${declaration}\``);
}

describe('External links settings row placement', () => {
  const stripped = stripComments(SOURCE);

  it('is rendered from the Links section', () => {
    expect(functionBody(SOURCE, 'function linksSection()')).toContain('externalLinksSection()');
    expect(functionBody(SOURCE, 'function renderSubview()')).toContain(`case 'links': return linksSection();`);
  });

  it('is NOT rendered from Experimental, whose nav entry is desktop-only', () => {
    // `if (key === 'experimental') return isTauri();` filters the row out of the
    // nav on every browser and PWA, so anything nested there is unreachable
    // exactly where this setting applies. That is the bug this file exists for.
    expect(functionBody(SOURCE, 'function experimentalSection()')).not.toContain('externalLinksSection()');
  });

  it('keeps the Experimental nav entry desktop-gated, so the premise stays true', () => {
    // If this gate ever widens, revisit the placement above rather than silently
    // relying on a comment that no longer holds.
    expect(stripped).toContain(`if (key === 'experimental') return isTauri();`);
  });

  it('lists the Links nav entry on exactly the platforms that have a row for it', () => {
    // Same predicate as the row, so the entry can never appear over an empty
    // panel, nor hide a row that would have rendered.
    expect(stripped).toContain(`if (key === 'links') return externalLinkTargetConfigurable();`);
  });

  it('still gates the row itself on the platform where the choice bites', () => {
    expect(functionBody(SOURCE, 'function externalLinksSection()'))
      .toContain('if (!externalLinkTargetConfigurable()) return null;');
  });
});
