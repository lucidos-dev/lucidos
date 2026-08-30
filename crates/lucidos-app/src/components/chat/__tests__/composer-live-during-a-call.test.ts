/**
 * Speech and typing interleave in one transcript, so a call must not take the
 * composer away (the voice plan's decision 2).
 *
 * A source scan, because the composer cannot be rendered in isolation and the
 * property is structural. The composer can only lock itself out if something
 * in it reads the call, so the check is that nothing does. `PromptInput` knows
 * nothing about voice at all; the toggle in the controls row is the whole of
 * what a call puts in the prompt area.
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
  it('tells PromptInput nothing about a call', () => {
    const source = read('PromptInput.tsx');
    const imported = [...source.matchAll(/^import \{([^}]*)\} from '[^']*(voice|Call)[^']*';$/gm)]
      .flatMap((m) => m[1].split(',').map((s) => s.trim()))
      .filter(Boolean);
    expect(imported).toEqual([]);
  });

  it('never reads the call state, so it cannot branch on one', () => {
    expect(read('PromptInput.tsx')).not.toContain('voiceCall');
  });

  /** The row mounts the toggle and tells it whether this destination can take
   *  a call (ADR 0165). What it must never read is the call's own STATE: that
   *  is what would let a live call reach back into the composer. */
  it('gives the controls row the toggle and nothing else about a call', () => {
    const source = read('PromptRowControls.tsx');
    expect(source).toContain('<CallToggle available=');
    expect(source).not.toContain('voiceCall');
  });

  it('leaves the textarea and Send untouched by a call', () => {
    expect(read('CallToggle.tsx')).not.toMatch(/disabled|prompt-textarea|readOnly/);
  });
});

describe('a call captions itself nowhere', () => {
  it('draws no panel over the prompt area', () => {
    expect(read('PromptInput.tsx')).not.toMatch(/call-strip|CallStrip/);
  });

  it('keeps the call state announceable, which is the one thing a panel did', () => {
    const source = read('CallToggle.tsx');
    expect(source).toContain('role="status"');
    expect(source).toContain('callStatusLabel(phase)');
  });
});
