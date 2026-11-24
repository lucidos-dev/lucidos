import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { REDUCED_MOTION_ROOT, cssRules, isReducedMotionRule, type CssRule } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(resolve(here, '../chat/input-messages.css'), 'utf8');
const rules = cssRules(css);
const toneRules = cssRules(readFileSync(resolve(here, '../global/host-components.css'), 'utf8'));

const PHASES = ['connecting', 'listening', 'speaking', 'ending'] as const;

function forPhase(phase: string): CssRule[] {
  return rules.filter(
    (r) => r.selector.includes('call-toggle') && r.selector.includes(`"${phase}"`),
  );
}

function stillRules(phase: string): CssRule[] {
  return forPhase(phase).filter((r) => isReducedMotionRule(r));
}

function movingRules(phase: string): CssRule[] {
  return forPhase(phase).filter((r) => !isReducedMotionRule(r));
}

function stillLook(phase: string): string {
  const seen = new Map<string, string>();
  for (const rule of [...movingRules(phase), ...stillRules(phase)]) {
    for (const [prop, value] of rule.props) {
      if (prop === 'animation') continue;
      seen.set(`${rule.selector.replace(/\[data-call-phase[^\]]*\]/, '')} ${prop}`, value);
    }
  }
  return [...seen]
    .sort()
    .map(([k, v]) => `${k}: ${v}`)
    .join('\n');
}

describe('every call phase has a paint of its own', () => {
  for (const phase of PHASES) {
    it(`${phase} is styled at all`, () => {
      const expected = phase === 'listening' ? 0 : 1;
      expect(forPhase(phase).length).toBeGreaterThanOrEqual(expected);
    });
  }

  it('paints the two transitional phases differently from each other', () => {
    expect(stillLook('connecting')).not.toBe(stillLook('ending'));
  });

  it('paints neither transitional phase like a live call', () => {
    for (const phase of ['connecting', 'ending']) {
      expect(stillLook(phase), phase).not.toBe(stillLook('listening'));
      expect(stillLook(phase), phase).not.toBe(stillLook('speaking'));
    }
  });

  it('changes no box on a floor flip', () => {
    const geometry = ['width', 'height', 'padding', 'margin', 'border-width', 'font-size'];
    for (const phase of ['listening', 'speaking']) {
      for (const rule of forPhase(phase)) {
        for (const prop of geometry) {
          expect(rule.props.get(prop), `${rule.selector} sets ${prop}`).toBeUndefined();
        }
      }
    }
  });
});

describe('a reader who stopped the motion still sees four phases', () => {
  it('leaves each transitional phase something static to be told apart by', () => {
    const looks = PHASES.map(stillLook);
    expect(new Set(looks).size, looks.join('\n---\n')).toBe(PHASES.length);
  });

  it('tells the transitional phases apart by brightness as well as by colour', () => {
    for (const phase of ['connecting', 'ending']) {
      expect(stillLook(phase), `${phase} rests at the same brightness as a live call`)
        .toContain('opacity');
    }
    const rest = (phase: string): string | undefined =>
      forPhase(phase).find((r) => r.props.has('opacity'))?.props.get('opacity');
    expect(rest('connecting')).not.toBe(rest('ending'));
  });

  it('draws no ring around the handset, which ADR 0207 settled', () => {
    const drawn = rules.filter(
      (r) => r.selector.includes('call-toggle') && r.selector.includes('::'),
    );
    expect(drawn.map((r) => r.selector)).toEqual([]);
  });

  it('turns off every animation it declares', () => {
    for (const phase of PHASES) {
      const animated = movingRules(phase).filter(
        (r) => (r.props.get('animation') ?? 'none') !== 'none',
      );
      for (const rule of animated) {
        const stopped = stillRules(phase).some(
          (r) => r.selector === `${REDUCED_MOTION_ROOT} ${rule.selector}` && r.props.get('animation') === 'none',
        );
        expect(stopped, `${rule.selector} keeps moving under reduced motion`).toBe(true);
      }
    }
  });

  it('declares its animations as bare literals, which the speed slider skips', () => {
    for (const phase of PHASES) {
      for (const rule of movingRules(phase)) {
        expect(rule.props.get('animation') ?? '').not.toContain('--duration');
      }
    }
  });

  it('references keyframes this sheet declares', () => {
    const named = PHASES.flatMap((p) => movingRules(p))
      .map((r) => r.props.get('animation')?.split(/\s+/)[0])
      .filter((name): name is string => Boolean(name));
    expect(named.length, 'no phase animates anything').toBeGreaterThan(0);
    for (const name of named) {
      expect(css, `@keyframes ${name} is never declared`).toContain(`@keyframes ${name}`);
    }
  });
});

describe('a connect that dwells stops claiming progress', () => {
  const dwelt = rules.filter(
    (r) => r.selector.includes('call-toggle') && r.selector.includes('data-call-wait'),
  );

  it('is painted at all', () => {
    expect(dwelt.length, 'the dwell changes the copy and nothing else').toBeGreaterThan(0);
  });

  it('stops the pulse, because we are waiting on the reader', () => {
    const glyph = dwelt.filter((r) => r.selector.includes('svg'));
    expect(glyph.length).toBe(1);
    expect(glyph[0].props.get('animation')).toBe('none');
  });

  it('holds the glyph brighter than the pulse rests it', () => {
    const dwelling = dwelt.find((r) => r.selector.includes('svg'))?.props.get('opacity');
    const connecting = movingRules('connecting')
      .find((r) => r.selector.includes('svg'))
      ?.props.get('opacity');
    expect(dwelling, 'the dwelt glyph names no brightness').toBeDefined();
    expect(Number(dwelling)).toBeGreaterThan(Number(connecting));
  });

  it('takes the waiting tone, not the caution one', () => {
    const tinted = dwelt.filter((r) => r.props.has('color'));
    expect(tinted.length).toBeGreaterThan(0);
    for (const rule of tinted) {
      expect(rule.props.get('color'), rule.selector).toContain('--accent-notable');
    }
  });

  it('names no hover of its own, so the pointer keeps its one promise', () => {
    expect(dwelt.filter((r) => r.selector.includes(':hover'))).toEqual([]);
  });
});

describe('the colour says what is happening, the pointer what a press does', () => {
  const live = toneRules.find((r) => r.selector === '.icon-btn[aria-pressed="true"][data-tone="live"]');

  const hovers = rules.filter(
    (r) => r.selector.includes('call-toggle') && r.selector.includes(':hover'),
  );

  it('paints a call that is up green, the whole way through', () => {
    expect(live?.props.get('color'), 'the live paint names no colour').toContain('--accent-green');
    expect(live?.props.get('background')).toContain('--accent-green');
  });

  it('keeps red off every resting phase', () => {
    const resting = rules.filter(
      (r) => r.selector.includes('call-toggle') && !r.selector.includes(':hover'),
    );
    for (const rule of resting) {
      for (const [prop, value] of rule.props) {
        expect(value, `${rule.selector} rests on red (${prop})`).not.toContain('--accent-red');
      }
    }
  });

  it('promises the hang-up under the pointer, and nowhere else', () => {
    const dead = hovers.find((r) => r.selector.includes('"ending"'));
    const living = hovers.filter((r) => !r.selector.includes('"ending"'));
    expect(living.length, 'no live phase answers the pointer').toBeGreaterThan(0);
    for (const rule of living) {
      const paint = [...rule.props.values()].join(' ');
      expect(paint, `${rule.selector} offers no hang-up`).toContain('--accent-red');
    }
    expect(dead?.props.get('color'), 'the spent control promises a hang-up').toContain(
      '--text-muted',
    );
  });

  it('declares the shared hover after every phase it has to beat', () => {
    const shared = hovers.find((r) => !r.selector.includes('data-call-'));
    const painted = rules.filter(
      (r) =>
        r.selector.includes('call-toggle') &&
        r.selector.includes('data-call-') &&
        !r.selector.includes(':hover') &&
        (r.props.has('color') || r.props.has('background')),
    );
    expect(shared, 'no shared hover rule at all').toBeDefined();
    expect(painted.length, 'no phase paints itself').toBeGreaterThan(0);
    expect(rules.indexOf(shared!)).toBeGreaterThan(rules.indexOf(painted[painted.length - 1]));
  });
});

describe('the ending control looks as dead as it behaves', () => {
  it('drops the live colour for the muted tone', () => {
    const tinted = forPhase('ending').filter((r) => r.props.has('color'));
    expect(tinted.length, 'ending never restates its colour').toBeGreaterThan(0);
    for (const rule of tinted) {
      expect(rule.props.get('color'), rule.selector).toContain('--text-muted');
    }
  });

  it('offers no pointer affordance', () => {
    const cursors = forPhase('ending')
      .map((r) => r.props.get('cursor'))
      .filter(Boolean);
    expect(cursors).toContain('default');
  });

  it('does not brighten under the pointer', () => {
    const hovered = forPhase('ending').filter((r) => r.selector.includes(':hover'));
    expect(hovered.length, 'ending takes the base hover back').toBe(1);
    expect(hovered[0].props.get('color')).toContain('--text-muted');
  });
});
