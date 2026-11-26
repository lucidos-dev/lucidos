// @vitest-environment jsdom
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
  readFileSync(resolve(repo, 'crates/lucidos-core/src/core/themes/theme-tokens.json'), 'utf-8'),
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
  it('are the seven marks tokens, host-only and never derived from the seeds', () => {
    const marks = catalog.tokens.filter(t => t.group === 'marks');
    expect(marks.map(t => t.name)).toEqual([
      '--lucidos-mark-bg-top',
      '--lucidos-mark-bg-bottom',
      '--lucidos-mark-fg',
      '--claude-mark',
      '--codex-mark',
      '--agent-icon-theme',
      '--agent-icon-muted',
    ]);
    for (const token of marks) {
      expect(token.frames, token.name).toBe(false);
      expect(token.derive, token.name).toBeUndefined();
    }
  });

  it('keep the Claude mark on --initiator-coding-agent and Codex on the primary text colour by default', () => {
    expect(defaultOf('--claude-mark')).toBe('var(--initiator-coding-agent)');
    expect(defaultOf('--codex-mark')).toBe('var(--text-primary)');
  });

  it('keep the Theme look on the accent and the Muted look on the muted text colour by default', () => {
    expect(defaultOf('--agent-icon-theme')).toBe('var(--accent)');
    expect(defaultOf('--agent-icon-muted')).toBe('var(--text-muted)');
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
  it('fills --codex-mark through a presentation attribute', () => {
    const host = document.createElement('div');
    render(<CodexIcon />, host);
    const svg = host.querySelector('svg')!;
    expect(svg.getAttribute('fill')).toBe(withFallback('--codex-mark'));
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


describe('the agent icon colors setting', () => {
  const rules = cssRules(readFileSync(resolve(styles, 'components.css'), 'utf-8'));
  const props = (selector: string) => {
    const rule = rules.find(r => r.selector === selector);
    expect(rule, selector).toBeDefined();
    return rule!.props;
  };
  const flatLooks = ':root:is([data-agent-icon-colors="theme"], [data-agent-icon-colors="muted"]) ';
  const iconSlots = ':is(.thread-row-agent, .response-executor-icon, .initiator-icon, .nav-history-icon)';

  it('maps each flat look to its own theme token on the icon itself, so scoped theme overrides reach it', () => {
    const flatTargets = ':is(.thread-row-agent, .response-executor-icon, .initiator-icon, .nav-history-icon, .lucidos-agent-flat)';
    expect(props(`:root[data-agent-icon-colors="theme"] ${flatTargets}`).get('--agent-icon-flat')).toBe('var(--agent-icon-theme)');
    expect(props(`:root[data-agent-icon-colors="muted"] ${flatTargets}`).get('--agent-icon-flat')).toBe('var(--agent-icon-muted)');
  });

  it('recolors Claude and Codex in every agent icon slot under a flat look', () => {
    const slots = props(flatLooks + iconSlots);
    expect(slots.get('--claude-mark')).toBe('var(--agent-icon-flat)');
    expect(slots.get('--codex-mark')).toBe('var(--agent-icon-flat)');
  });

  it('swaps the Lucidos tile for the flat mark under a flat look only', () => {
    expect(props('.lucidos-agent-flat').get('display')).toBe('none');
    expect(props('.lucidos-agent-flat').get('color')).toBe('var(--agent-icon-flat)');
    expect(props(flatLooks + '.lucidos-agent-flat').get('display')).toBe('inline-block');
    expect(props(flatLooks + '.lucidos-agent-tile').get('display')).toBe('none');
  });

  it('leaves Brand to the brand tokens, with no rule of its own', () => {
    const sheets = styleSheetPaths(styles).map(path => readFileSync(path, 'utf-8'));
    const brandRules = sheets.flatMap(css => cssRules(css).filter(r => r.selector.includes('"brand"')));
    expect(brandRules).toEqual([]);
  });

  it('colors drawer marks from the brand tokens, like the thread pane', () => {
    const drawer = cssRules(readFileSync(resolve(styles, 'drawer.css'), 'utf-8'));
    expect(drawer.find(r => r.selector === '.thread-row-agent .claude-icon')!.props.get('color')).toBe('var(--claude-mark)');
    expect(drawer.filter(r => /\.codex-icon|\.lucidos-mark/.test(r.selector))).toEqual([]);
  });
});


describe('default agent mark contrast', () => {
  const colour = (name: string, mode: 'light' | 'dark'): string => {
    const value = catalog.tokens.find(t => t.name === name)!.default[mode];
    const reference = /^var\((--[a-z-]+)\)$/.exec(value);
    return reference ? colour(reference[1], mode) : value;
  };
  const luminance = (hex: string): number => {
    const digits = hex.slice(1);
    const full = digits.length === 3 ? [...digits].map(c => c + c).join('') : digits;
    const channels = full.match(/../g)!.map(c => {
      const value = parseInt(c, 16) / 255;
      return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
    });
    return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
  };
  it.each(['light', 'dark'] as const)('keeps coloured and muted glyphs readable in %s mode', mode => {
    for (const mark of ['--claude-mark', '--codex-mark', '--text-secondary']) {
      for (const surface of ['--bg-primary', '--bg-secondary', '--bg-selected', '--bg-tertiary']) {
        const a = luminance(colour(mark, mode));
        const b = luminance(colour(surface, mode));
        expect((Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05), `${mark} on ${surface}`).toBeGreaterThanOrEqual(3);
      }
    }
  });
});
