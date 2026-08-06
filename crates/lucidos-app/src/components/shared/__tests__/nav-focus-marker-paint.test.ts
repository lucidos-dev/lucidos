import { describe, it, expect } from 'vitest';
// @ts-expect-error Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error same
import { dirname, resolve } from 'node:path';
// @ts-expect-error same
import { fileURLToPath } from 'node:url';

import { NAV_FOCUS_FADE_MS, NAV_FOCUS_RAMP_MS } from '../focusMarker';

const here: string = dirname(fileURLToPath(import.meta.url));
/** Comments stripped up front: this file's rules are heavily commented, and a
 *  comment sitting between two declarations otherwise breaks the "a declaration
 *  starts after a `;`" assumption below (and would let prose satisfy a scan). */
const stripComments = (source: string): string => source.replace(/\/\*[\s\S]*?\*\//g, '');
const css = stripComments(
  readFileSync(resolve(here, '../../../styles/global/host-components.css'), 'utf-8'),
);
/** The marker's paint lives here, but the COLOUR it mixes is a theme token defined
 *  over in base.css, so that file has to be read too (see the last test). */
const baseCss = stripComments(
  readFileSync(resolve(here, '../../../styles/global/base.css'), 'utf-8'),
);

/** The `{…}` block for a TOP-LEVEL rule with exactly this selector. Two things keep
 *  it exact, and they are different mechanisms: the `\s*\{` right after the selector
 *  is what stops `.nav-focus-stuck` also matching `.nav-focus-stuck.nav-focus-fading`
 *  (both start at column 0), while anchoring at column 0 is what stops it swallowing
 *  the INDENTED same-selector overrides inside the reduced-motion media query, which
 *  would otherwise fold that block's `animation: none` into the base rule's
 *  declarations and quietly invert the bloom assertion. */
function block(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const re = new RegExp(`^${escaped}\\s*\\{([^}]*)\\}`, 'gm');
  const hits = [...css.matchAll(re)].map(m => m[1]);
  return hits.join('\n');
}

function declaration(source: string, property: string): string | undefined {
  const escaped = property.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return source.match(new RegExp(`(?:^|;)\\s*${escaped}\\s*:\\s*([^;]+)`, 'm'))?.[1].trim();
}

/** A `box-shadow`'s layers. Splitting on every `,` would also split inside
 *  `color-mix(in srgb, …, transparent)`, so only depth-0 commas separate layers. */
function layers(shadow: string | undefined): string[] {
  const out: string[] = [];
  let depth = 0;
  let current = '';
  for (const ch of shadow ?? '') {
    if (ch === '(') depth++;
    else if (ch === ')') depth--;
    if (ch === ',' && depth === 0) {
      out.push(current.trim());
      current = '';
      continue;
    }
    current += ch;
  }
  if (current.trim()) out.push(current.trim());
  return out;
}

// The navigation focus marker is a Slack-style BACKGROUND HIGHLIGHT with a spotlight
// glow, not the accent outline FRAME it replaced (which read as a heavy dialog border
// around a landed chat turn). These assertions pin that shape. They are deliberately
// about the PAINT only. The marker's geometry (the four-side uniform-gap rules per
// host) is pinned separately by nav-focus-marker-uniform-gap.test.ts and
// turn-header-gutter.test.ts, and none of those numbers changed with the repaint.
describe('nav focus marker paint: background highlight, not a frame', () => {
  it('fills the box and glows past it, all on one box-shadow', () => {
    const stuck = block('.nav-focus-stuck');
    expect(stuck).not.toBe('');
    const shadow = declaration(stuck, 'box-shadow');
    expect(shadow).toBeDefined();
    // The fill: an inset layer with a viewport-sized spread paints the whole padding
    // box. Without `inset` there is no background highlight at all, only a ring.
    expect(shadow).toMatch(/inset\s+0\s+0\s+0\s+100vmax/);
    // The glow: blurred OUTER layers bleeding past the edge. Counting the non-inset
    // layers is what separates "a flat tint" from "a spotlight".
    const outerLayers = layers(shadow).filter(l => !l.includes('inset'));
    expect(outerLayers.length).toBeGreaterThanOrEqual(2);
    expect(outerLayers.every(l => /0\s+0\s+[\d.]+rem/.test(l))).toBe(true);
  });

  it('paints no visible frame', () => {
    const stuck = block('.nav-focus-stuck');
    expect(declaration(stuck, 'border')).toBeUndefined();
    // The one permitted outline is the forced-colors fallback below: transparent, so
    // it paints nothing in normal rendering. A coloured outline here is the frame
    // coming back.
    const outline = declaration(stuck, 'outline');
    expect(outline).toBe('0.125rem solid transparent');
    // A `background` here would clobber the host's own background. .list-row's hover
    // tint has to keep showing under a marked plugin row, which is exactly why the
    // fill is an inset shadow (it paints above the background, below the content).
    expect(declaration(stuck, 'background')).toBeUndefined();
    expect(declaration(stuck, 'background-color')).toBeUndefined();
    expect(declaration(stuck, 'background-image')).toBeUndefined();
  });

  it('survives forced-colors mode via the transparent outline', () => {
    // High-contrast mode STRIPS box-shadow. A marker made only of shadows would leave
    // those users with no indication of where the navigation landed at all, the one
    // thing the old outline frame did give them. The transparent outline is forced to
    // a system colour there. Same fallback every focus ring in shared-components.css
    // carries; deleting it as "dead CSS" is the regression this guards.
    expect(declaration(block('.nav-focus-stuck'), 'outline')).toContain('transparent');
  });

  it('derives every colour from --nav-focus-glow, never from --accent', () => {
    const stuck = block('.nav-focus-stuck');
    const colours = [...stuck.matchAll(/color-mix\([^)]*\)/g)].map(m => m[0]);
    expect(colours.length).toBeGreaterThan(0);
    // One token for both themes, and it must be the marker's OWN token.
    expect(colours.every(c => c.includes('var(--nav-focus-glow)'))).toBe(true);
    // The regression this pins, and the reason a whole token exists for one marker:
    // the wash used to be --accent at 16%, which is --picked-surface's hue at nearly
    // its alpha (--accent at 12%, worn by the user bubble, the selected question
    // option and the collapsed-turn stub). A spotlit turn therefore read as a PICKED
    // surface, not as a marker, and it could not have read otherwise, since the
    // header, links and badges are all accent-blue too. Note `var(--accent)` and not
    // just `--accent`: --accent-yellow and friends would substring-match, and the
    // marker must not reach for those either (--accent-yellow is caution-only).
    expect(stuck).not.toMatch(/var\(--accent[-)]/);
    // No hand-picked literals anywhere in the marker's paint.
    expect(stuck).not.toMatch(/#[0-9a-fA-F]{3,8}\b/);
  });

  it('has that token defined by BOTH themes, since a missing one erases the marker', () => {
    // This is a cross-file coupling with a silent failure mode, which is why it is
    // worth a test. `color-mix(in srgb, var(--undefined) 16%, transparent)` is not a
    // no-op and does not fall back: the unresolved var makes the color-mix invalid at
    // computed-value time, which takes the WHOLE box-shadow down with it. Drop the
    // token from one theme and the marker doesn't merely lose its tint there, it
    // stops painting at all, in that theme only. Every other assertion in this file
    // reads host-components.css and would stay green through that.
    const themeBlock = (selector: string): string => {
      // No nested braces inside a theme block, so `[^}]*` is the whole body. Anchored
      // at column 0 so a longer selector (html.ios-pwa[data-theme="dark"], which
      // overrides only the header blues) can't be mistaken for the base theme block.
      const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
      const body = baseCss.match(new RegExp(`^${escaped}\\s*\\{([^}]*)\\}`, 'm'))?.[1];
      expect(body, `no top-level \`${selector}\` block in base.css`).toBeDefined();
      return body ?? '';
    };
    // A literal colour, not merely "defined": the assertion above about the marker
    // block would be trivially satisfiable by `--nav-focus-glow: var(--accent)`,
    // which passes every scan in this file while painting the exact accent wash the
    // recolour removed. Requiring a hex here closes that back door, and the marker
    // block's own no-literals rule is what keeps the hex on this side of the split.
    for (const selector of ['html, html[data-theme="dark"]', 'html[data-theme="light"]']) {
      expect(declaration(themeBlock(selector), '--nav-focus-glow')).toMatch(
        /^#[0-9a-fA-F]{3,8}$/,
      );
    }
  });

  it('turns on by rising to full and STOPPING there, with nothing to sag back from', () => {
    const stuck = block('.nav-focus-stuck');
    const animation = declaration(stuck, 'animation');
    expect(animation).toMatch(/^nav-focus-spotlight-on\s/);
    // Pinned against the constant, not a literal: focusMarker.ts starts the hold when
    // the ramp ENDS, so it has to know this duration. If the two drift, the hold spends
    // part of itself on the ramp and delivers less than the full brightness time it
    // documents.
    expect(Number(animation?.match(/([\d.]+)s/)?.[1]) * 1000).toBe(NAV_FOCUS_RAMP_MS);
    const keyframes = css.match(/@keyframes\s+nav-focus-spotlight-on\s*\{([\s\S]*?)\n\}/)?.[1];
    expect(keyframes).toBeDefined();

    // It starts from NOTHING: every layer at 0% is transparent, which is what makes
    // it read as a light being switched on rather than as a highlight that was always
    // there.
    const zeroStop = keyframes?.match(/0%\s*\{([^}]*)\}/)?.[1] ?? '';
    expect(zeroStop).not.toBe('');
    // Count first: `layers()` of a declaration this scanner can't find is [], and
    // `.every()` over [] is true, so moving the fill to another property would make
    // this assertion report green while checking nothing. `not.toBe('')` only proves
    // the keyframe BODY parsed.
    expect(layers(declaration(zeroStop, 'box-shadow')).length).toBe(3);
    expect(layers(declaration(zeroStop, 'box-shadow')).every(l => l.includes('transparent'))).toBe(
      true,
    );

    // ...and it ONLY RISES. This is the assertion that matters most in this file: an
    // earlier cut overshot to a surge partway through and settled back into the
    // resting values, which lost a third of the brightness a quarter-second after
    // landing and was reported as the light immediately turning back down. A ramp with
    // no intermediate stop cannot dim after arriving. (A stop BELOW resting would also
    // be monotonic, but "no intermediate stop" is the simplest form of the rule to
    // state and to keep, so that is what is pinned.)
    expect(keyframes?.match(/\d+%\s*\{|\b(?:from|to)\s*\{/g)).toEqual(['0% {']);
    expect(keyframes).not.toMatch(/color-mix/);

    // No explicit `to`: the animation settles into the element's own computed value,
    // i.e. the base rule, so the resting state is written once and cannot drift.
    // A fill-mode would freeze the animated value over it and break that.
    expect(declaration(stuck, 'animation-fill-mode')).toBeUndefined();
    expect(animation).not.toMatch(/forwards|both/);

    // The 0% stop must match the base rule's layer count AND its inset-ness, or the
    // layers cannot interpolate: box-shadow list interpolation pads the shorter list
    // with a non-inset shadow, the per-layer inset flags then mismatch, and the whole
    // animation silently goes DISCRETE (the marker pops on instead of ramping).
    const restingShadow = declaration(stuck, 'box-shadow');
    const insets = (shadow: string | undefined) =>
      layers(shadow).map(l => l.includes('inset')).join(',');
    expect(layers(declaration(zeroStop, 'box-shadow')).length).toBe(layers(restingShadow).length);
    expect(insets(declaration(zeroStop, 'box-shadow'))).toBe(insets(restingShadow));
  });

  it('dissolves via an ANIMATION so it still runs when it interrupts the turn-on', () => {
    const fading = block('.nav-focus-stuck.nav-focus-fading');
    expect(fading).not.toBe('');

    // The dismiss must be an animation, not a transition. This is correctness, not
    // taste: while the turn-on animation runs it owns box-shadow, so a transition
    // underneath never starts, and cancelling the turn-on to free the property just
    // lands the value on the after-change style in the same frame with nothing to
    // interpolate. Measured on the real rule, that shape made a dismiss inside the
    // ramp BLINK the marker off within two frames in Chromium. Replacing the
    // running animation by name plays deterministically in both engines.
    expect(declaration(fading, 'transition')).toBeUndefined();
    const animation = declaration(fading, 'animation');
    expect(animation).toMatch(/^nav-focus-spotlight-off\s/);
    // `forwards` holds the end state for the sliver between the animation finishing
    // and focusMarker.ts stripping the classes.
    expect(animation).toMatch(/\bforwards\b/);
    // `linear`, not an ease: over this long an ease-out dumps most of the alpha up
    // front and drags an invisible tail, which reads as a quick fade plus a lag.
    expect(animation).toMatch(/\blinear\b/);

    // No `from` stop: the implicit one is the element's underlying value, i.e. the
    // base rule, so the resting state stays written exactly once and the dissolve
    // cannot drift from it.
    const off = css.match(/@keyframes\s+nav-focus-spotlight-off\s*\{([\s\S]*?)\n\}/)?.[1];
    expect(off).toBeDefined();
    expect(off).not.toMatch(/0%\s*\{|\bfrom\s*\{/);
    const toStop = off?.match(/(?:to|100%)\s*\{([^}]*)\}/)?.[1] ?? '';
    const endShadow = declaration(toStop, 'box-shadow');
    const restingShadow = declaration(block('.nav-focus-stuck'), 'box-shadow');
    // Same layer count and inset-ness as the base rule, or the layers cannot
    // interpolate and the marker would pop away instead of dissolving.
    expect(layers(endShadow).length).toBe(layers(restingShadow).length);
    expect(layers(endShadow).map(l => l.includes('inset')).join(',')).toBe(
      layers(restingShadow).map(l => l.includes('inset')).join(','),
    );
    expect(layers(endShadow).every(l => l.includes('transparent'))).toBe(true);

    // focusMarker.ts removes the classes on a timer; the CSS duration must match it or
    // the dissolve is cut off (timer shorter) or the class lingers (timer longer).
    const seconds = Number(animation?.match(/([\d.]+)s/)?.[1]);
    expect(seconds * 1000).toBe(NAV_FOCUS_FADE_MS);
    // Slower than the turn-on: the marker is being retired because the user moved on,
    // not cleared out of their way, so it drains rather than cutting. Pin the
    // relationship rather than the number, so either duration can be retuned and keep
    // it.
    const onSeconds = Number(
      declaration(block('.nav-focus-stuck'), 'animation')?.match(/([\d.]+)s/)?.[1],
    );
    expect(seconds).toBeGreaterThan(onSeconds);
    // And only slower. This one IS an absolute ceiling rather than a ratio, because
    // the thing it guards is a human-scale judgement about the dismiss and not a
    // relationship to the ramp: the dismiss is triggered BY the action that moves the
    // user on, so a dissolve running past about a second reads as the marker being
    // slow to let go rather than as a graceful exit. It shipped at 2.5s and was
    // reported exactly that way; the prose above the rule invites lengthening it back.
    expect(seconds).toBeLessThanOrEqual(1);
  });

  it('drops both the turn-on and the dissolve under reduced motion', () => {
    const query = css.match(/@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/)?.[1] ?? '';
    expect(query).toMatch(/\.nav-focus-stuck\s*\{[^}]*animation:\s*none/);
    // The fading arm needs its OWN rule here. It is a two-class selector, so it
    // outranks the bare `.nav-focus-stuck` one above and would keep animating.
    expect(query).toMatch(/\.nav-focus-stuck\.nav-focus-fading\s*\{[^}]*animation:\s*none/);

    // Both overrides TIE on specificity with the rules they override (0-1-0 against
    // 0-1-0, 0-2-0 against 0-2-0), so the only thing that makes them win is coming
    // later in the file. Existence alone is not the contract: hoist this block above
    // the base rules and reduced motion silently breaks while the assertions above
    // stay green.
    const mediaAt = css.indexOf('@media (prefers-reduced-motion');
    expect(mediaAt).toBeGreaterThan(-1);
    expect(mediaAt).toBeGreaterThan(css.search(/^\.nav-focus-stuck\s*\{/m));
    expect(mediaAt).toBeGreaterThan(css.search(/^\.nav-focus-stuck\.nav-focus-fading\s*\{/m));
    // And exactly one such block, or the one this test read may not be the one that
    // wins.
    expect(css.match(/@media \(prefers-reduced-motion/g)?.length).toBe(1);
  });
});
