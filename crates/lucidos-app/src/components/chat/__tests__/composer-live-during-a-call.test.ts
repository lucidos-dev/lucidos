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

  it('gives the controls row the toggle and nothing else about a call', () => {
    const source = read('PromptRowControls.tsx');
    expect(source).toContain('callToggleAction(callIsOffered(thread, codingAgent))');
    expect(source).not.toContain('voiceCall');
  });

  it('leaves the textarea and Send untouched by a call', () => {
    const source = read('CallToggle.tsx');
    expect(source).not.toMatch(/prompt-textarea|readOnly/);
    expect(source.replace(/aria-disabled/g, '')).not.toMatch(/disabled/);
  });
});

describe('a call captions itself nowhere', () => {
  it('draws no panel over the prompt area', () => {
    expect(read('PromptInput.tsx')).not.toMatch(/call-strip|CallStrip/);
  });

  it('keeps the call state announceable, which is the one thing a panel did', () => {
    const source = read('CallToggle.tsx');
    expect(source).toContain('role="status"');
    expect(source).toContain('callStatusLabel(call)');
  });
});
