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

const markup: string = source
  .split('\n')
  .filter((line: string) => {
    const trimmed = line.trim();
    return !trimmed.startsWith('//') && !trimmed.startsWith('*') && !trimmed.startsWith('/*');
  })
  .join('\n');

function dirPickerRule(): string {
  const match = /\.dir-picker\s*\{([^}]*)\}/.exec(css);
  if (!match) throw new Error('.dir-picker has no rule: was it renamed?');
  return match[1];
}

describe('DirectoryPicker is a modal', () => {
  it('renders its panel through <Overlay> with the shared backdrop container', () => {
    expect(markup).toMatch(/<Overlay\b[\s\S]{0,200}?panelClass="[^"]*\bdir-picker"/);
    expect(markup).not.toMatch(/backdrop=\{false\}/);
  });

  it('carries no `.confirm-overlay` wrapper, whose rule is long gone', () => {
    expect(markup).not.toContain('confirm-overlay');
  });

  it('leaves the panel unplaced, so the container is what centres it', () => {
    expect(dirPickerRule()).not.toMatch(/position\s*:\s*(fixed|absolute)/);
  });
});
