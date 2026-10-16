// @vitest-environment jsdom
/**
 * A theme recolours the three actor icons through the "Brand marks" tokens.
 *
 * `theme-token-catalog.test.ts` already pins each default to `base.css`. This
 * pins the other half. Every mark reads its token, and each var() fallback is
 * the catalog default. So a mark painted before the stylesheet loads looks the
 * same as one painted after.
 */
import { describe, it, expect } from 'vitest';
import { render } from 'preact';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, relative, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { LucidosMark } from '../../components/shared/LucidosMark';
import { CodexIcon } from '../../components/shared/icons';
import { cssRules, styleSheetPaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const styles: string = resolve(here, '..');
const repo: string = resolve(here, '../../../../..');

interface CatalogToken {
  name: string;
  group: string;
  frames: boolean;
  default: { dark: string; light: string };
  derive?: unknown;
}
const catalog: { tokens: CatalogToken[] } = JSON.parse(
  readFileSync(resolve(repo, 'crates/lucidos-engine/src/core/themes/theme-tokens.json'), 'utf-8'),
);
const defaultOf = (name: string): string => {
  const token = catalog.tokens.find(t => t.name === name);
  expect(token, `${name} is not in the catalog`).toBeDefined();
  expect(token!.default.light, `${name} has one default for both modes`).toBe(token!.default.dark);
  return token!.default.dark;
};
const withFallback = (name: string): string => `var(${name}, ${defaultOf(name)})`;

function renderMarks(background: boolean): SVGSVGElement[] {
  const host = document.createElement('div');
  render(<><LucidosMark background={background} /><LucidosMark background={background} /></>, host);
  return [...host.querySelectorAll('svg')] as SVGSVGElement[];
}

describe('the brand mark tokens', () => {
  it('are the five marks tokens, host-only and never derived from the seeds', () => {
    const marks = catalog.tokens.filter(t => t.group === 'marks');
    expect(marks.map(t => t.name)).toEqual([
      '--lucidos-mark-bg-top',
      '--lucidos-mark-bg-bottom',
      '--lucidos-mark-fg',
      '--claude-mark',
      '--codex-mark',
    ]);
    for (const token of marks) {
      expect(token.frames, token.name).toBe(false);
      expect(token.derive, token.name).toBeUndefined();
    }
  });

  it('keep the Claude mark on --initiator-coding-agent and Codex on the light accent by default', () => {
    expect(defaultOf('--claude-mark')).toBe('var(--initiator-coding-agent)');
    expect(defaultOf('--codex-mark')).toBe('var(--accent-light)');
  });
});

describe('the Lucidos mark', () => {
  it('paints its gradient stops from the tile tokens', () => {
    const [svg] = renderMarks(true);
    const stops = [...svg.querySelectorAll('stop')].map(s => s.getAttribute('stop-color'));
    expect(stops).toEqual([withFallback('--lucidos-mark-bg-top'), withFallback('--lucidos-mark-bg-bottom')]);
  });

  it('paints its glyph from --lucidos-mark-fg as an attribute, tile or no tile', () => {
    for (const background of [true, false]) {
      const [svg] = renderMarks(background);
      const glyph = svg.querySelector('.lmk-tile')!.parentElement!;
      expect(glyph.getAttribute('fill')).toBe(withFallback('--lucidos-mark-fg'));
      expect(glyph.getAttribute('style')).toBeNull();
    }
  });

  it('keeps one gradient id per instance', () => {
    const marks = renderMarks(true);
    const ids = marks.map(svg => svg.querySelector('radialGradient')!.id);
    expect(new Set(ids).size).toBe(2);
    marks.forEach((svg, i) => {
      expect(svg.querySelector('.lmk-bg')!.getAttribute('fill')).toBe(`url(#${ids[i]})`);
    });
  });
});

describe('the Codex mark', () => {
  it('strokes --codex-mark through a presentation attribute', () => {
    // `.commands-btn .codex-icon` repaints it with an author rule, which an
    // inline style would beat.
    const host = document.createElement('div');
    render(<CodexIcon />, host);
    const svg = host.querySelector('svg')!;
    expect(svg.getAttribute('stroke')).toBe(withFallback('--codex-mark'));
    expect(svg.getAttribute('style')).toBeNull();
  });
});

describe('the Claude mark', () => {
  const sheets: { file: string; css: string }[] = styleSheetPaths(styles)
    .map(path => ({ file: relative(styles, path), css: readFileSync(path, 'utf-8') }));

  it('takes --claude-mark in every rule that colours it', () => {
    const tinted = sheets.flatMap(({ file, css }) =>
      cssRules(css)
        .filter(r => /\.claude-icon\b/.test(r.selector) && r.props.has('color'))
        .map(r => ({ where: `${file}: ${r.selector}`, color: r.props.get('color') })),
    );
    expect(tinted.length).toBeGreaterThan(0);
    for (const { where, color } of tinted) expect(color, where).toBe('var(--claude-mark)');
  });

  it('is the only reader of --initiator-coding-agent, so a theme setting --claude-mark wins everywhere', () => {
    const readers = sheets
      .filter(({ css }) => css.includes('var(--initiator-coding-agent)'))
      .map(({ file }) => file);
    expect(readers).toEqual(['global/base.css']);
  });
});
