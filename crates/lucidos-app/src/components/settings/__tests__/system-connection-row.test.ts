import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, decl } from '../../../styles/__tests__/css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const source: string = readFileSync(resolve(here, '../SystemPage.tsx'), 'utf-8');
const systemCss: string = readFileSync(
  resolve(here, '../../../styles/settings/system.css'),
  'utf-8',
);

describe('the Connection block reads the shared notice', () => {
  it('takes both halves from the one table', () => {
    expect(source).toContain("import { connectionNotice } from '../../utils/connectionNotice'");
    expect(source).toContain("const notice = connectionNotice(noticeStatus, name, 'full');");
    expect(source).toContain('{notice ? notice.title : \'Connected\'}');
    expect(source).toContain('<p class="system-status-note">{notice.detail}</p>');
  });

  it('keeps its own word for the one state the table is silent on', () => {
    expect(source).not.toMatch(/status === 'connecting' \? 'Connecting/);
  });

  it('hides the dot from assistive tech, since the word is right beside it', () => {
    expect(source).toContain('<span class={`status-dot ${noticeStatus}`} aria-hidden="true" />');
  });
});

describe('the explanation lands under the state word', () => {
  it('indents by derivation, not by a guess', () => {
    const indent = decl(block(systemCss, '.system-status-note {'), 'padding') ?? '';
    expect(indent).toContain('var(--system-status-dot)');
    expect(indent).toContain('var(--system-status-gap)');
    const rootTokens = block(systemCss, ':root');
    for (const name of ['--system-status-dot', '--system-status-gap']) {
      expect(decl(rootTokens, name), `${name} is read but declared nowhere`).not.toBeNull();
    }
  });

  it('reads as the explanation rather than as a second heading', () => {
    const note = block(systemCss, '.system-status-note {');
    expect(decl(note, 'font-size')).toBe('var(--font-size-sm)');
    expect(decl(note, 'font-weight')).toBe('400');
    expect(decl(note, 'color')).toBe('var(--text-secondary)');
  });

  it('joins the row into one block instead of hanging off it', () => {
    expect(decl(block(systemCss, '.system-status-row.has-note {'), 'padding-bottom')).toBe('0');
    expect(source).toContain("`system-status-row${notice ? ' has-note' : ''}`");
  });
});
