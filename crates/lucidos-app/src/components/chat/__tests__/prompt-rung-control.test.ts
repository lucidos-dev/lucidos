import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const promptSource: string = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');

describe('the rung control in the prompt row', () => {
  it('leaves the send button standing in compose too', () => {
    expect(promptSource).toContain("const sendButton = morphMode !== 'hidden' ? (");
  });

  it('keeps the pill disabled over a pending question or permission card', () => {
    expect(promptSource).not.toContain('const showsRungControl');
    expect(promptSource).toContain('disabledReason={rungDisabledReason}');
  });

  it('hands the cluster members and the action slot to the right-hand group', () => {
    expect(promptSource).toContain(
      '<PromptActionsRight cluster={members.cluster.map(renderMember)} action={actionSlot} />',
    );
  });

  it('folds the pill in its place in the one row order', () => {
    expect(promptSource).toContain('rung: {');
    expect(promptSource).not.toContain('foldActions.unshift(');
    expect(promptSource).toContain('rowAttrs={foldAttrs(RUNG_FOLD_KEY)}');
  });
});
