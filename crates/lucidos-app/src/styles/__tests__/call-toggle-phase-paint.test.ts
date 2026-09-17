/**
 * Each call phase paints itself, and the reduced-motion reader still sees four.
 *
 * The component test beside it (`components/chat/__tests__/
 * call-toggle-shows-its-phase.test.tsx`) proves the phase reaches the button.
 * This is the other half: that the stylesheet does something distinct with it.
 * Nothing else in the gate can see that. `tsc` never reads CSS and `vite build`
 * fails only on syntax, so a phase with no rule at all builds perfectly clean.
 *
 * Three properties, and the third is the one that decays quietly. A phase told
 * apart by motion ALONE collapses into its neighbour for anybody who asked
 * their system to stop moving. A caller reported exactly that: one control
 * that looked the same in two states.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { cssRules, type CssRule } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(resolve(here, '../chat/input-messages.css'), 'utf8');
const rules = cssRules(css);

/** The four phases a call is in when the control is on screen and not idle. */
const PHASES = ['connecting', 'listening', 'speaking', 'ending'] as const;

/** Every rule whose selector names this phase of the call toggle. */
function forPhase(phase: string): CssRule[] {
  return rules.filter(
    (r) => r.selector.includes('call-toggle') && r.selector.includes(`"${phase}"`),
  );
}

/** The same rules, split by whether reduced motion is in force. */
function stillRules(phase: string): CssRule[] {
  return forPhase(phase).filter((r) => r.atRules.includes('prefers-reduced-motion'));
}

function movingRules(phase: string): CssRule[] {
  return forPhase(phase).filter((r) => !r.atRules.includes('prefers-reduced-motion'));
}

/**
 * What a phase looks like once the motion is taken away.
 *
 * Declarations from the ordinary rules, then the reduced-motion ones on top,
 * with every `animation` dropped. Source order stands in for the cascade. That
 * holds here: the reduced-motion block is last, and every selector in the group
 * carries the same weight.
 */
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
      // `listening` is the exception that proves the rule: it takes the base
      // live paint and adds nothing, so it is the one phase with no rule of
      // its own. Every other phase must declare something.
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

  /** The floor flip must not resize the button, or the prompt actions row
   *  reflows every time either side starts talking. */
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
    // The whole point of the scan. Strip the animations and the four looks must
    // still differ. A phase carried by motion alone reads as its neighbour the
    // moment the system setting is on.
    const looks = PHASES.map(stillLook);
    expect(new Set(looks).size, looks.join('\n---\n')).toBe(PHASES.length);
  });

  /** Colour alone is a thin distinction, and it is no distinction at all for a
   *  reader who cannot separate green from grey. */
  it('tells the transitional phases apart by shape as well as by colour', () => {
    const ring = (phase: string): boolean =>
      forPhase(phase).some((r) => r.selector.includes('::after'));
    expect(ring('connecting'), 'a call being placed draws no ring').toBe(true);
    expect(ring('ending'), 'a call running down draws one').toBe(false);
  });

  it('closes the connecting ring once the sweep stops', () => {
    // A sweep frozen part-way round reads as a glitch. A complete circle reads
    // as a deliberate outline, which is the shape above.
    const still = stillRules('connecting').find((r) => r.selector.includes('::after'));
    expect(still?.props.get('border-color')).toBe('currentColor');
  });

  it('turns off every animation it declares', () => {
    for (const phase of PHASES) {
      // A rule that already says `none` has nothing to turn off: the dwelt
      // connect stops its own sweep, whatever the system setting says.
      const animated = movingRules(phase).filter(
        (r) => (r.props.get('animation') ?? 'none') !== 'none',
      );
      for (const rule of animated) {
        const stopped = stillRules(phase).some(
          (r) => r.selector === rule.selector && r.props.get('animation') === 'none',
        );
        expect(stopped, `${rule.selector} keeps moving under reduced motion`).toBe(true);
      }
    }
  });

  it('declares its animations as bare literals, which the speed slider skips', () => {
    // An activity indicator is not a transition, so it carries no
    // `--duration-*` token. See .claude/rules/frontend-css.md.
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
  /** Every rule for the dwelt connect, which the component marks with its own
   *  attribute rather than a phase of its own. */
  const dwelt = rules.filter(
    (r) => r.selector.includes('call-toggle') && r.selector.includes('data-call-wait'),
  );

  it('is painted at all', () => {
    expect(dwelt.length, 'the dwell changes the copy and nothing else').toBeGreaterThan(0);
  });

  it('stops the sweep, because we are waiting on the reader', () => {
    const ring = dwelt.filter((r) => r.selector.includes('::after'));
    expect(ring.length).toBe(1);
    expect(ring[0].props.get('animation')).toBe('none');
  });

  /** A still ring is the plain connect's reduced-motion shape, so the dwelt one
   *  needs a second difference or the two collapse for that reader. */
  it('leaves the ring a different shape from the one still being drawn', () => {
    const ring = dwelt.find((r) => r.selector.includes('::after'));
    expect(ring?.props.get('border-style')).toBe('dashed');
  });

  it('takes the waiting tone, not the caution one', () => {
    // `--accent-yellow` is the caution tone in this palette, and a browser
    // asking for permission is not a caution. See global/base.css.
    const tinted = dwelt.filter((r) => r.props.has('color'));
    expect(tinted.length).toBeGreaterThan(0);
    for (const rule of tinted) {
      expect(rule.props.get('color'), rule.selector).toContain('--accent-notable');
    }
  });

  it('keeps that tone under the pointer', () => {
    const hovered = dwelt.filter((r) => r.selector.includes(':hover'));
    expect(hovered.length, 'the connect hover hands the green back').toBe(1);
    expect(hovered[0].props.get('color')).toContain('--accent-notable');
  });
});

describe('the ending control looks as dead as it behaves', () => {
  it('drops the live red for the muted tone', () => {
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

  /** `.icon-btn`'s own hover would hand the live red back, which is the one
   *  thing a spent control must not offer. */
  it('does not brighten under the pointer', () => {
    const hovered = forPhase('ending').filter((r) => r.selector.includes(':hover'));
    expect(hovered.length, 'ending takes the base hover back').toBe(1);
    expect(hovered[0].props.get('color')).toContain('--text-muted');
  });
});
