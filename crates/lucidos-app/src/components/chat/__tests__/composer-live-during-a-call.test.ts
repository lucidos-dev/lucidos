/**
 * Speech and typing interleave in one transcript, so a call must not take the
 * composer away (the voice plan's decision 2).
 *
 * A source scan, because the composer cannot be rendered in isolation and the
 * property is structural. The composer can only lock itself out if something
 * in it reads the call, so the check is that nothing does. The strip is the
 * one voice thing `PromptInput` knows about, and it sits above the box rather
 * than in place of it.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { fileURLToPath } from 'node:url';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const CHAT = resolve(here, '..');

function read(name: string): string {
  return readFileSync(resolve(CHAT, name), 'utf8');
}

describe('a call leaves the composer alone', () => {
  it('gives PromptInput the strip and nothing else about a call', () => {
    const source = read('PromptInput.tsx');
    const imported = [...source.matchAll(/^import \{([^}]*)\} from '[^']*(voice|Call)[^']*';$/gm)]
      .flatMap((m) => m[1].split(',').map((s) => s.trim()))
      .filter(Boolean);
    expect(imported).toEqual(['CallStrip']);
  });

  it('never reads the call state, so it cannot branch on one', () => {
    expect(read('PromptInput.tsx')).not.toContain('voiceCall');
  });

  it('puts the strip above the box, never in place of it', () => {
    const source = read('PromptInput.tsx');
    expect(source.indexOf('<CallStrip />')).toBeGreaterThan(-1);
    expect(source.indexOf('<CallStrip />')).toBeLessThan(source.indexOf('class="prompt-box"'));
  });

  it('gives the controls row the toggle and nothing else about a call', () => {
    const source = read('PromptRowControls.tsx');
    expect(source).toContain('<CallToggle />');
    expect(source).not.toContain('voiceCall');
  });

  it('leaves the textarea and Send untouched by a call', () => {
    for (const name of ['CallToggle.tsx', 'CallStrip.tsx']) {
      expect(read(name)).not.toMatch(/disabled|prompt-textarea|readOnly/);
    }
  });
});
