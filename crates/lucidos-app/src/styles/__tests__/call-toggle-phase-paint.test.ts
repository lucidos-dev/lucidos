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
   *  reader who cannot separate green from grey. Brightness carries it, since
   *  a drawn mark is ruled out (ADR 0207). */
  it('tells the transitional phases apart by brightness as well as by colour', () => {
    for (const phase of ['connecting', 'ending']) {
      expect(stillLook(phase), `${phase} rests at the same brightness as a live call`)
        .toContain('opacity');
    }
    // And not at the SAME brightness, or the two collapse into each other.
    const rest = (phase: string): string | undefined =>
      forPhase(phase).find((r) => r.props.has('opacity'))?.props.get('opacity');
    expect(rest('connecting')).not.toBe(rest('ending'));
  });

  it('draws no ring around the handset, which ADR 0207 settled', () => {
    // The owner refused one twice, so the control itself pulses instead. A
    // pseudo-element under this selector is how a ring comes back.
    const drawn = rules.filter(
      (r) => r.selector.includes('call-toggle') && r.selector.includes('::'),
    );
    expect(drawn.map((r) => r.selector)).toEqual([]);
  });

  it('turns off every animation it declares', () => {
    for (const phase of PHASES) {
      // A rule that already says `none` has nothing to turn off: the dwelt
      // connect stops its own pulse, whatever the system setting says.
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

  it('stops the pulse, because we are waiting on the reader', () => {
    const glyph = dwelt.filter((r) => r.selector.includes('svg'));
    expect(glyph.length).toBe(1);
    expect(glyph[0].props.get('animation')).toBe('none');
  });

  /** A still glyph is the plain connect's reduced-motion look too, so the dwelt
   *  one needs a second difference or the two collapse for that reader. */
  it('holds the glyph brighter than the pulse rests it', () => {
    const dwelling = dwelt.find((r) => r.selector.includes('svg'))?.props.get('opacity');
    const connecting = movingRules('connecting')
      .find((r) => r.selector.includes('svg'))
      ?.props.get('opacity');
    expect(dwelling, 'the dwelt glyph names no brightness').toBeDefined();
    expect(Number(dwelling)).toBeGreaterThan(Number(connecting));
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

  it('names no hover of its own, so the pointer keeps its one promise', () => {
    // A press here cancels the call, as it ends one in every other live
    // phase. So the dwell takes the shared red hover below (ADR 0209).
    expect(dwelt.filter((r) => r.selector.includes(':hover'))).toEqual([]);
  });
});

describe('the colour says what is happening, the pointer what a press does', () => {
  /** The live paint every non-idle phase starts from. */
  const live = rules.find(
    (r) => r.selector.endsWith('.active[data-role="call-toggle"]') && r.props.has('color'),
  );

  /** Every hover rule the call toggle declares, in source order. */
  const hovers = rules.filter(
    (r) => r.selector.includes('call-toggle') && r.selector.includes(':hover'),
  );

  it('paints a call that is up green, the whole way through', () => {
    expect(live?.props.get('color'), 'the live paint names no colour').toContain('--accent-green');
    expect(live?.props.get('background')).toContain('--accent-green');
  });

  it('keeps red off every resting phase', () => {
    // Red is the hang-up, and a call that is up is not a failure. A resting
    // rule reaching for it is what this test catches.
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
    // Every hover but the spent control's is red: a press there ends the call.
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
    // Same specificity, so source order decides. Declared earlier, the promise
    // would lose to the connect's and the dwell's own tones.
    const shared = hovers.find((r) => !r.selector.includes('data-call-'));
    // The rules it has to beat are the ones that PAINT a phase at rest. The
    // reduced-motion block below it only takes motion away.
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

  /** `.icon-btn`'s own hover would hand the live red back, which is the one
   *  thing a spent control must not offer. */
  it('does not brighten under the pointer', () => {
    const hovered = forPhase('ending').filter((r) => r.selector.includes(':hover'));
    expect(hovered.length, 'ending takes the base hover back').toBe(1);
    expect(hovered[0].props.get('color')).toContain('--text-muted');
  });
});
