/**
 * The "…" cut marker on a step line is muted text in every theme.
 *
 * A cut point is not a state. With a status token such as `--accent-notable`,
 * it turned amber in the green theme, so every step line read as a warning.
 * The scan covers every stylesheet, so a theme cannot repaint the marker either.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { rulesTargeting, styleSheetPaths } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const STYLES = resolve(here, '..');

const colored = styleSheetPaths(STYLES).flatMap((path) =>
  rulesTargeting(readFileSync(path, 'utf8'), 'ellipsis-marker')
    .filter((r) => r.props.has('color'))
    .map((r) => ({ path, selector: r.selector, color: r.props.get('color') })),
);

describe('the ellipsis marker', () => {
  it('has a colour rule', () => {
    expect(colored.length).toBeGreaterThan(0);
  });

  it('is painted only with --text-muted', () => {
    const off = colored.filter((r) => r.color !== 'var(--text-muted)');
    expect(off, 'every rule colouring .ellipsis-marker must use var(--text-muted)').toEqual([]);
  });
});
