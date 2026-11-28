import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const promptSource = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');
const composerCss = readFileSync(
  resolve(here, '../../../styles/chat/input-messages.css'),
  'utf-8',
);
const mobileCss = readFileSync(resolve(here, '../../../styles/mobile.css'), 'utf-8');
const headerActionsSource = readFileSync(
  resolve(here, '../../layout/headerActions.tsx'),
  'utf-8',
);

function at(needle: string, from = 0): number {
  const i = promptSource.indexOf(needle, from);
  if (i < 0) throw new Error(`${needle} not found in PromptInput.tsx`);
  return i;
}

function textRowSpan(): string {
  const open = at('<div class="prompt-row">');
  return promptSource.slice(open, at('<div class={rowClass}', open));
}

function clearSpec(): string {
  const span = /const clearDraftAction: HeaderActionSpec \| null = hasText \? \{\n\s+key: 'prompt-clear'[\s\S]*?\n {2}\} : null;/.exec(promptSource);
  if (!span) throw new Error('no clearDraftAction spec in PromptInput.tsx');
  return span[0];
}

function clearRuleBodies(css: string): string[] {
  const bodies: string[] = [];
  const selector = /(^|[\s,{}])\.prompt-clear\b[^{}]*\{([^}]*)\}/g;
  let match: RegExpExecArray | null;
  const source = css.replace(/\/\*[\s\S]*?\*\//g, '');
  while ((match = selector.exec(source)) !== null) bodies.push(match[2]);
  return bodies;
}

describe('the composer has one control on its right edge', () => {
  it('renders no button in the text row', () => {
    expect(textRowSpan()).not.toMatch(/<button\b/);
  });

  it('renders the fold cluster inside .prompt-actions-row', () => {
    expect(at('<OverflowMenu')).toBeGreaterThan(at('<div class={rowClass}'));
  });

  it('places the fold cluster ahead of the right-hand action group', () => {
    expect(at('<OverflowMenu')).toBeLessThan(at('<PromptActionsRight '));
  });

  it('folds from the fold order and renders the row order', () => {
    expect(promptSource).toContain('const foldActions = members.foldOrder;');
    expect(at('<OverflowMenu')).toBeLessThan(at('{members.row.map(renderMember)}'));
    expect(at('{members.row.map(renderMember)}')).toBeLessThan(at('<PromptActionsRight '));
  });

  it('hands the clear action to the one row order', () => {
    expect(promptSource).toMatch(/clearDraft: clearDraftAction,/);
  });
});

describe('the clear button is one of the prompt row icons', () => {
  it('wears the same box and glyph classes as its neighbours', () => {
    expect(clearSpec()).toMatch(/extraClass: 'prompt-clear'/);
    expect(headerActionsSource).toMatch(/header-icon\$\{a\.extraClass/);
    expect(headerActionsSource).toMatch(/thread-overflow-item\$\{a\.extraClass/);
  });

  it('is measured by the fold', () => {
    expect(promptSource).toMatch(
      /const foldAttrs = \(key: string\) => \(\{ 'data-row-item': 'fold', \[FOLD_KEY_ATTR\]: key \}\);/,
    );
    expect(promptSource).toMatch(/triggerAttrs=\{MORE_TRIGGER_ATTRS\}/);
  });

  it('renders only while there is a draft to clear', () => {
    expect(clearSpec()).toMatch(/= hasText \? \{/);
    expect(clearSpec()).not.toMatch(/\binvisible\b/);
  });

  it('declares no size or colour of its own, on any viewport', () => {
    const bodies = [...clearRuleBodies(composerCss), ...clearRuleBodies(mobileCss)];
    for (const body of bodies) {
      expect(
        body,
        `.prompt-clear must inherit .icon-btn.header-icon, but a rule sets: ${body.trim()}`,
      ).not.toMatch(/\b(width|height|color|padding|margin|align-self|font-size)\b/);
    }
  });
});
