import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

// Where the rung control sits in the prompt row. It is a pill beside the send
// button in every state, so every send path is the send button's own. Before
// the first send the pill folds into the ⋯ menu LAST, so the model and effort
// stay one tap away. In a started thread it folds FIRST, so the change actions
// and toggles outrank it.
const here: string = dirname(fileURLToPath(import.meta.url));
const promptSource: string = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');

describe('the rung control in the prompt row', () => {
  it('leaves the send button standing in compose too', () => {
    expect(promptSource).toContain("const sendButton = morphMode !== 'hidden' ? (");
  });

  it('draws no pill over a pending question or permission card', () => {
    expect(promptSource).toContain('const showsRungControl = !answersCard;');
  });

  it('sits in the right-hand cluster, before the send button', () => {
    const cluster = promptSource.slice(promptSource.indexOf('<div class="prompt-actions-right">'));
    expect(cluster.indexOf('<LazyRungControl')).toBeGreaterThan(-1);
    expect(cluster.indexOf('<LazyRungControl')).toBeLessThan(cluster.indexOf('{answersCard ? answerControl : sendButton}'));
  });

  it('leads the cluster, so an action such as Apply or Archive holds the right edge', () => {
    const cluster = promptSource.slice(promptSource.indexOf('<div class="prompt-actions-right">'));
    expect(cluster.indexOf('<LazyRungControl')).toBeLessThan(cluster.indexOf('bannerActions.filter(standing)'));
  });

  it('folds the pill into the ⋯ menu last before the first send, and first after it', () => {
    const fold = promptSource.slice(promptSource.indexOf('const foldActions'), promptSource.indexOf('const clusterKeys'));
    expect(fold).toContain('if (showsRungControl) {');
    expect(fold).toContain('if (inComposeContext) foldActions.push(rungAction);\n    else foldActions.unshift(rungAction);');
    // Nothing joins the order after it.
    expect(fold.slice(fold.indexOf('const rungAction'))).not.toMatch(/foldActions\.push\((?!rungAction\))/);
    expect(promptSource).toContain('rowAttrs={foldAttrs(RUNG_FOLD_KEY)}');
  });
});
