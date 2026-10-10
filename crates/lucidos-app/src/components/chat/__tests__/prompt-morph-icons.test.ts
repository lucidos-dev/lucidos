import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

// The compose Send→Cancel morph renders icons, not text labels: a round
// up-arrow for Send and a stop-square while a turn runs/cancels. This pins
// the icon swap (no regression back to the "Send"/"Cancel" text labels) while
// keeping the accessibility contract (aria-label) and the e2e selector hooks
// (class names) intact.
const here: string = dirname(fileURLToPath(import.meta.url));
const promptSource = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');

function findMorphButton(): string {
  const match = promptSource.match(/<button\s+key="send-cancel-morph"[\s\S]*?<\/button>/);
  if (!match) throw new Error('send-cancel-morph button not found in PromptInput.tsx');
  return match[0];
}

describe('send-cancel-morph renders icons, not text labels', () => {
  it('imports SendArrowIcon and StopIcon from shared icons', () => {
    expect(promptSource).toMatch(/import\s*\{[^}]*\bSendArrowIcon\b[^}]*\bStopIcon\b[^}]*\}\s*from\s*['"]\.\.\/shared\/icons['"]/);
  });

  it('renders <SendArrowIcon /> for the send state', () => {
    expect(findMorphButton()).toMatch(/<SendArrowIcon\s*\/>/);
  });

  it('renders <StopIcon /> for the cancel/canceling state', () => {
    expect(findMorphButton()).toMatch(/<StopIcon\s*\/>/);
  });

  it('no longer renders the visible "Cancel..." text label (icon-only now)', () => {
    const btn = findMorphButton();
    // The old visible-label ternary emitted 'Cancel...' for the canceling
    // state — a string that never appears in the aria-label/tooltip, so its
    // absence is a clean signal the text labels are gone.
    expect(btn).not.toMatch(/'Cancel\.\.\.'/);
    // The button children must be icons, not a text-label ternary.
    expect(btn).toMatch(/>\s*\{\/\*[\s\S]*?\*\/\}\s*\{morphMode/);
  });

  it('keeps the round-shape class and the e2e selector classes', () => {
    const btn = findMorphButton();
    expect(btn).toMatch(/send-cancel-morph send-cancel-round/);
  });

  it('stays blue in the stop/cancel state (no red danger variant)', () => {
    // The stop button matches the send button's blue .action-btn — it no
    // longer swaps to the red action-btn-danger while a turn is running.
    expect(findMorphButton()).not.toMatch(/action-btn-danger/);
  });

  it('preserves the aria-label contract: "Send message" vs "Cancel"', () => {
    const btn = findMorphButton();
    expect(btn).toMatch(/aria-label=\{[^}]*'Cancel'[^}]*'Send message'/);
  });
});
