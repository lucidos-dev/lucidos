import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { cssRules, rulesTargeting, selectorList } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(resolve(here, '../global/host-components.css'), 'utf8');
const chatCss = readFileSync(resolve(here, '../chat/input-messages.css'), 'utf8');
const baseCss = readFileSync(resolve(here, '../global/base.css'), 'utf8');
const groupCss = readFileSync(resolve(here, '../skills.css'), 'utf8');
const icons = readFileSync(resolve(here, '../../components/shared/icons.tsx'), 'utf8');
const chatExchange = readFileSync(resolve(here, '../../components/chat/ChatExchange.tsx'), 'utf8');
const groupHeader = readFileSync(resolve(here, '../../components/triggers/TriggerGroupHeader.tsx'), 'utf8');

const BANDS = ['header-icon', 'row-icon', 'inline-icon'];

const hostRules = cssRules(css);

const rule = hostRules.find(r =>
  selectorList(r.selector).includes('.icon-btn.header-icon .trash-icon'),
);

describe('the trash is sized by its ink', () => {
  it('has a rule that names both measurements', () => {
    expect(rule, 'no .icon-btn.header-icon .trash-icon rule').toBeDefined();
    const pencil = Number(rule!.props.get('--pencil-ink'));
    const trash = Number(rule!.props.get('--trash-ink'));
    expect(pencil, '--pencil-ink is not a number').toBeGreaterThan(0);
    expect(trash, '--trash-ink is not a number').toBeGreaterThan(0);
    expect(pencil).toBeLessThanOrEqual(24);
    expect(trash).toBeLessThan(pencil);
  });

  it('grows the box by the ratio and takes the same ratio out of the stroke', () => {
    for (const prop of ['width', 'height']) {
      expect(rule!.props.get(prop), `${prop} is not derived from the two fills`)
        .toMatch(/calc\(var\(--icon-glyph\) \* var\(--pencil-ink\) \/ var\(--trash-ink\)\)/);
    }
    expect(rule!.props.get('stroke-width'), 'the stroke does not divide by the same ratio')
      .toMatch(/calc\(2 \* var\(--trash-ink\) \/ var\(--pencil-ink\)\)/);
  });

  it('applies to the trash in every band', () => {
    for (const band of BANDS) {
      expect(selectorList(rule!.selector), `.${band}'s trash is not corrected`)
        .toContain(`.icon-btn.${band} .trash-icon`);
    }
  });

  it('is aimed at artwork that still carries the class and the stroke it assumes', () => {
    const trashIcon = /export function TrashIcon\(\)[\s\S]*?\n}/.exec(icons)?.[0] ?? '';
    expect(trashIcon, 'TrashIcon not found in icons.tsx').not.toBe('');
    expect(trashIcon, 'the glyph lost the class the rule selects').toContain('class="trash-icon"');
    expect(trashIcon, 'the artwork is no longer drawn at stroke-width 2')
      .toContain('stroke-width="2"');
  });
});

const steps = new Map<string, number>();
for (const r of cssRules(baseCss)) {
  for (const [prop, value] of r.props) {
    if (prop.startsWith('--icon-size-') && /^[\d.]+rem$/.test(value)) {
      steps.set(prop, parseFloat(value));
    }
  }
}

const bandRule = hostRules.find(r => {
  const members = selectorList(r.selector);
  return BANDS.every(band => members.includes(`.icon-btn.${band}`));
});

const boxRule = hostRules.find(r => {
  const members = selectorList(r.selector);
  return members.includes('.icon-btn.header-icon')
    && members.includes('.icon-btn.row-icon')
    && !members.includes('.icon-btn.inline-icon')
    && r.props.has('width');
});

const overlayRule = hostRules.find(r =>
  selectorList(r.selector).includes('.icon-btn.inline-icon::before'),
);

const inlineChipRule = hostRules.find(r =>
  selectorList(r.selector).includes('.icon-btn.inline-icon') && r.props.has('padding'),
);

const bandNominals = new Map(
  BANDS.map(band => [
    band,
    rulesTargeting(css, band).filter(r => r.props.has('--icon-glyph')),
  ]),
);

const step = (value: string) => /^var\((--icon-size-[\w-]+)\)$/.exec(value)?.[1];

const callerRules = [
  ...rulesTargeting(chatCss, 'queued-message-remove'),
  ...rulesTargeting(groupCss, 'trigger-group-delete'),
  ...rulesTargeting(groupCss, 'trigger-group-rename'),
];

describe('the nominal glyph size is declared per band, on the shared tap target', () => {
  it('keeps the tap target on one rule and puts a nominal on each class it names', () => {
    expect(bandRule, 'no rule shares a declaration across all three bands').toBeDefined();
    const root = cssRules(baseCss).find(r => r.selector === ':root' && r.props.has('--icon-tap-target'));
    expect(root?.props.get('--icon-tap-target'), 'the shared target is not 2.25rem').toBe('2.25rem');
    expect(bandRule!.props.has('--icon-tap-target'), 'the band rule shadows the root target').toBe(false);
    expect(bandRule!.props.has('--icon-glyph'), 'the shared rule sets one nominal for every band').toBe(false);

    for (const band of BANDS) {
      const found = bandNominals.get(band)!;
      expect(found.length, `.${band} does not declare exactly one nominal`).toBe(1);
      expect(selectorList(bandRule!.selector), `.${band}'s nominal is not on a shared tap target`)
        .toContain(found[0].selector);
      expect([...found[0].props.keys()], `"${found[0].selector}" sets more than the nominal`)
        .toEqual(['--icon-glyph']);
    }

    for (const band of BANDS) {
      const sel = `.icon-btn.${band} svg`;
      const svg = hostRules.find(r => selectorList(r.selector).includes(sel));
      expect(svg, `no ${sel} rule`).toBeDefined();
      expect(svg!.props.get('width'), `${sel} is not sized from the nominal`).toBe('var(--icon-glyph)');
      expect(svg!.props.get('height')).toBe('var(--icon-glyph)');
    }
  });

  it('gives the target to every band, as a box in a strip and a tall overlay in text', () => {
    expect(boxRule, 'the two chrome bands no longer share a box rule').toBeDefined();
    expect(overlayRule, 'the inline band has no tap-target overlay').toBeDefined();
    for (const prop of ['width', 'height']) {
      expect(boxRule!.props.get(prop), `the box does not take its ${prop} from the target`)
        .toBe('var(--icon-tap-target)');
    }
    expect(overlayRule!.props.get('height'), 'the overlay does not take its height from the target')
      .toBe('var(--icon-tap-target)');
    expect(overlayRule!.props.get('width'), 'the overlay reaches sideways, into the words')
      .toBe('100%');
    expect(overlayRule!.props.get('content'), 'the overlay is not generated').toBe("''");
    expect(overlayRule!.props.get('position'), 'the overlay is in flow').toBe('absolute');
    expect(overlayRule!.props.get('left'), 'the overlay has no horizontal offset').toBe('50%');
    expect(overlayRule!.props.get('top'), 'the overlay has no vertical offset').toBe('50%');
    expect(overlayRule!.props.get('transform'), 'the overlay is not pulled back onto its centre')
      .toBe('translate(-50%, -50%)');

    expect(inlineChipRule, 'the inline band declares no chip padding').toBeDefined();
    for (const prop of ['width', 'height', 'min-width', 'min-height']) {
      expect(inlineChipRule!.props.has(prop), `the inline band sets ${prop} on its box`).toBe(false);
    }
    expect(inlineChipRule!.props.get('padding'), 'the chip padding is not one named value')
      .toBe('var(--inline-icon-chip)');
    expect(inlineChipRule!.props.get('margin'), 'the chip is not handed back on both axes')
      .toBe('calc(-1 * var(--inline-icon-chip))');
    for (const prop of ['margin-block', 'margin-inline', 'margin-top', 'margin-left']) {
      expect(inlineChipRule!.props.has(prop), `"${prop}" splits the handback in two`).toBe(false);
    }
  });

  it('raises the inline glyph off the line box and onto the cap band', () => {
    expect(inlineChipRule!.props.get('top'), 'the inline glyph carries no optical rise')
      .toMatch(/^-[\d.]+em$/);
    expect(bandRule!.props.get('position'), 'the rise has nothing to offset against')
      .toBe('relative');
    const rise = Math.abs(parseFloat(inlineChipRule!.props.get('top')!));
    expect(rise, 'the rise is large enough to read as a layout move').toBeLessThan(0.15);
  });

  it('starts the bands at the large step and takes one step down per band', () => {
    const named = BANDS.map(band => {
      const s = step(bandNominals.get(band)![0].props.get('--icon-glyph')!);
      expect(s, `the ${band} nominal is not an --icon-size-* step`).toBeDefined();
      expect(steps.get(s!), `${s} is not a rem step in base.css`).toBeGreaterThan(0);
      return s!;
    });
    expect(named[0], 'the header nominal moved off the large step').toBe('--icon-size-lg');
    for (let i = 1; i < named.length; i++) {
      expect(steps.get(named[i]), `${named[i]} is not smaller than ${named[i - 1]}`)
        .toBeLessThan(steps.get(named[i - 1])!);
    }
  });

  it('is the single source: no caller retunes it in its own sheet', () => {
    for (const r of callerRules) {
      expect(r.props.has('--icon-glyph'), `"${r.selector}" picked up a local nominal`).toBe(false);
      for (const prop of ['width', 'height', 'min-width', 'min-height', 'padding']) {
        expect(r.props.has(prop), `"${r.selector}" sets ${prop} on the tap target`).toBe(false);
      }
      for (const prop of r.props.keys()) {
        expect(prop.startsWith('padding-'), `"${r.selector}" sets ${prop} on the tap target`).toBe(false);
      }
    }

    expect(chatExchange, 'the queued button no longer carries the inline-icon class')
      .toContain('class="icon-btn inline-icon queued-message-remove exchange-status-glyph"');
    for (const name of ['trigger-group-rename', 'trigger-group-delete']) {
      expect(groupHeader, `${name} no longer carries the row-icon class`)
        .toContain(`class="icon-btn row-icon ${name}"`);
    }
  });
});
