import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const inputCss = readFileSync(resolve(here, '../../../styles/chat/input-messages.css'), 'utf-8');
const responseCss = readFileSync(resolve(here, '../../../styles/chat/response.css'), 'utf-8');
const baseCss = readFileSync(resolve(here, '../../../styles/global/base.css'), 'utf-8');

function getBlock(css: string, selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const re = new RegExp(`${escaped}\\s*\\{([^}]*)\\}`, 'g');
  return [...css.matchAll(re)].map(m => m[1]).join('\n');
}

function declarationValue(block: string, property: string): string | undefined {
  const escaped = property.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return block.match(new RegExp(`${escaped}\\s*:\\s*([^;]+)`))?.[1].trim();
}

function gapBelow(block: string): string | undefined {
  return block.match(/(?:^|[;{\s])margin-bottom\s*:\s*([^;]+)/)?.[1].trim();
}

describe('turn header gutter', () => {
  it('keeps actor and executor icons aligned with turn body content', () => {
    const initiatorHeader = getBlock(inputCss, '.initiator-header');
    const initiatorBody = getBlock(inputCss, '.initiator-body');
    const responseHeader = getBlock(responseCss, '.response-header');
    const responseContent = getBlock(responseCss, '.response-content');

    expect(declarationValue(initiatorHeader, 'padding-left')).toBe('var(--turn-body-inset)');
    expect(declarationValue(responseHeader, 'padding-left')).toBe('var(--turn-body-inset)');
    expect(declarationValue(initiatorBody, 'padding-left')).toBe('var(--turn-body-inset)');
    expect(declarationValue(responseContent, 'padding-left')).toBe('var(--turn-body-inset)');
  });

  it('keeps the gap under both turn headers on one value', () => {
    const initiatorGap = gapBelow(getBlock(inputCss, '.initiator-header'));
    expect(initiatorGap, '.initiator-header declares no gap under the header').toBeTruthy();
    expect(
      gapBelow(getBlock(responseCss, '.response-header')),
      'the two turn headers must carry the same gap under them',
    ).toBe(initiatorGap);
  });

  it('gives the panels a base right inset matching the content left inset', () => {
    const base = inputCss.match(/\.initiator-panel\s*,\s*\.response-panel\s*\{([^}]*)\}/)?.[1] ?? '';
    expect(base).not.toBe('');
    expect(declarationValue(base, 'padding')).toBe(
      'var(--turn-feed-pad-y) var(--turn-body-inset) var(--turn-feed-pad-y) 0',
    );
  });

  it('drops the feed rhythm below the last turn, where nothing follows it', () => {
    const re = /\.thread-feed > \.chat-exchange:last-child > ([^{]*)\{([^}]*)\}/;
    const [, selectorTail, block] = inputCss.match(re) ?? [];
    expect(block, 'no last-turn rule').toBeTruthy();
    expect(declarationValue(block!, 'padding-bottom')).toBe('0');

    for (const panel of ['initiator-panel', 'response-panel']) {
      expect(selectorTail, `.${panel} not covered`).toContain(`.${panel}:last-child`);
    }
    expect(selectorTail).toContain(':not(.nav-focus-stuck)');
  });

  it('starts the first turn right under the title, on its own feed padding', () => {
    const transcript = inputCss.match(/\n\.thread-content\s*\{([^}]*)\}/)?.[1] ?? '';
    const top = declarationValue(transcript, 'padding')?.split('\n')[0].trim();
    expect(top).toBe('var(--anchor-subpixel, 0px)');
    expect(inputCss).not.toMatch(/\.chat-exchange:first-child > \.(initiator|response)-panel/);

    const rem = (name: string) =>
      parseFloat(baseCss.match(new RegExp(`${name}:\\s*([\\d.]+)rem`))?.[1] ?? 'NaN');
    expect(rem('--turn-feed-pad-y') - rem('--turn-body-inset')).toBeGreaterThanOrEqual(
      rem('--nav-focus-reach'),
    );
  });

  it('gives the focus-marked panels a uniform gap on all four sides', () => {
    const re =
      /\.initiator-panel\.nav-focus-stuck\s*,\s*\.response-panel\.nav-focus-stuck\s*\{([^}]*)\}/;
    const block = responseCss.match(re)?.[1] ?? '';
    expect(block).not.toBe('');
    expect(declarationValue(block, 'padding')).toBe(
      'var(--turn-body-inset) var(--turn-body-inset) var(--turn-body-inset) 0',
    );
    expect(declarationValue(block, 'margin-right')).toBeUndefined();
    expect(declarationValue(block, 'margin-top')).toBe(
      'calc(var(--turn-feed-pad-y) - var(--turn-body-inset))',
    );
    expect(declarationValue(block, 'margin-bottom')).toBe(
      'calc(var(--turn-feed-pad-y) - var(--turn-body-inset))',
    );
    expect(responseCss).not.toMatch(/\.event-pulse|\.event-focus-stuck/);
  });

  it('gives the focus-marked whole turn (.chat-exchange) a uniform gap too', () => {
    const block = getBlock(responseCss, '.chat-exchange.nav-focus-stuck');
    expect(block).not.toBe('');
    expect(declarationValue(block, 'padding')).toBe('var(--turn-body-inset) 0');
    expect(declarationValue(block, 'margin-right')).toBeUndefined();
    expect(declarationValue(block, 'margin-top')).toBe(
      'calc(var(--turn-feed-pad-y) - var(--turn-body-inset))',
    );
    expect(declarationValue(block, 'margin-bottom')).toBe(
      'calc(var(--turn-feed-pad-y) - var(--turn-body-inset))',
    );
    const firstChild =
      responseCss.match(
        /\.chat-exchange\.nav-focus-stuck > \.initiator-panel:first-child\s*,\s*\.chat-exchange\.nav-focus-stuck > \.response-panel:first-child\s*\{([^}]*)\}/,
      )?.[1] ?? '';
    expect(declarationValue(firstChild, 'padding-top')).toBe('0');
    const lastChild =
      responseCss.match(
        /\.chat-exchange\.nav-focus-stuck > \.initiator-panel:last-child\s*,\s*\.chat-exchange\.nav-focus-stuck > \.response-panel:last-child\s*\{([^}]*)\}/,
      )?.[1] ?? '';
    expect(declarationValue(lastChild, 'padding-bottom')).toBe('0');
    const errorTurn = getBlock(responseCss, '.chat-exchange.nav-focus-stuck:has(> .exchange-error:last-child)');
    expect(errorTurn).not.toBe('');
    expect(declarationValue(errorTurn, 'margin-bottom')).toBe('calc(-1 * var(--turn-body-inset))');
  });
});
