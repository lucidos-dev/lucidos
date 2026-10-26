import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

// Where the rung control sits in the prompt row. It is a pill beside the send
// button in every state, so every send path is the send button's own. It folds
// in its place in the row order, after every icon on its left
// (`prompt-row-controls.test.tsx`).
const here: string = dirname(fileURLToPath(import.meta.url));
const promptSource: string = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');

describe('the rung control in the prompt row', () => {
  it('leaves the send button standing in compose too', () => {
    expect(promptSource).toContain("const sendButton = morphMode !== 'hidden' ? (");
  });

  it('draws no pill over a pending question or permission card', () => {
    expect(promptSource).toContain('const showsRungControl = !answersCard;');
  });

  it('renders the cluster members, the pill first, before the send button', () => {
    const cluster = promptSource.slice(promptSource.indexOf('<div class="prompt-actions-right">'));
    expect(cluster.indexOf('{members.cluster.map(renderMember)}')).toBeGreaterThan(-1);
    expect(cluster.indexOf('{members.cluster.map(renderMember)}'))
      .toBeLessThan(cluster.indexOf('{actionSlot}'));
  });

  it('folds the pill in its place in the one row order', () => {
    expect(promptSource).toContain('rung: showsRungControl ? {');
    expect(promptSource).not.toContain('foldActions.unshift(');
    expect(promptSource).toContain('rowAttrs={foldAttrs(RUNG_FOLD_KEY)}');
  });
});
