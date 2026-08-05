import { describe, it, expect } from 'vitest';
// @ts-expect-error Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error same
import { dirname, resolve } from 'node:path';
// @ts-expect-error same
import { fileURLToPath } from 'node:url';

import { NAV_FOCUS_FADE_MS } from '../focusMarker';

const here: string = dirname(fileURLToPath(import.meta.url));
/** Comments stripped up front: this file's rules are heavily commented, and a
 *  comment sitting between two declarations otherwise breaks the "a declaration
 *  starts after a `;`" assumption below (and would let prose satisfy a scan). */
const css = readFileSync(resolve(here, '../../../styles/global/host-components.css'), 'utf-8')
  .replace(/\/\*[\s\S]*?\*\//g, '');

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

  it('derives every colour from --accent, so both themes work off one token', () => {
    const stuck = block('.nav-focus-stuck');
    const colours = [...stuck.matchAll(/color-mix\([^)]*\)/g)].map(m => m[0]);
    expect(colours.length).toBeGreaterThan(0);
    expect(colours.every(c => c.includes('var(--accent)'))).toBe(true);
    // No hand-picked literals anywhere in the marker's paint.
    expect(stuck).not.toMatch(/#[0-9a-fA-F]{3,8}\b/);
  });

  it('turns on over half a second, ramping up from nothing and settling at rest', () => {
    const stuck = block('.nav-focus-stuck');
    const animation = declaration(stuck, 'animation');
    expect(animation).toMatch(/^nav-focus-spotlight-on\s+0\.5s\b/);
    const keyframes = css.match(/@keyframes\s+nav-focus-spotlight-on\s*\{([\s\S]*?)\n\}/)?.[1];
    expect(keyframes).toBeDefined();

    // It starts from NOTHING: every layer at 0% is transparent, which is what makes
    // it read as a spotlight switching on rather than as a highlight that was always
    // there. (The first version deliberately did the opposite, starting brighter and
    // only decaying, so the marker was never missable. The user asked for the real
    // turn-on instead; that is safe here ONLY because the marker then persists with
    // no timeout, so a look-away can miss the ramp but never the marker.)
    const zeroStop = keyframes?.match(/0%\s*\{([^}]*)\}/)?.[1] ?? '';
    expect(zeroStop).not.toBe('');
    // Count first, for the same reason spelled out at the surge below: `layers()` of a
    // declaration this scanner can't find is [], and `.every()` over [] is true, so
    // moving the fill to another property would make this assertion report green while
    // checking nothing. `not.toBe('')` only proves the keyframe BODY parsed.
    expect(layers(declaration(zeroStop, 'box-shadow')).length).toBe(3);
    expect(layers(declaration(zeroStop, 'box-shadow')).every(l => l.includes('transparent'))).toBe(
      true,
    );

    // No explicit `to`: the animation settles into the element's own computed value,
    // i.e. the base rule, so the resting state is written once and cannot drift.
    // A fill-mode would freeze the animated value over it and break that.
    expect(keyframes).not.toMatch(/100%\s*\{|\bto\s*\{/);
    expect(declaration(stuck, 'animation-fill-mode')).toBeUndefined();
    expect(animation).not.toMatch(/forwards|both/);

    // The surge: the mid stop overshoots the resting values on every layer, which is
    // the difference between a lamp coming up and a linear crossfade. Pin the layer
    // count FIRST: `alphas` reads integer percentages, so rewriting the colours in a
    // notation it doesn't match (a fractional `13.5%`, a relative colour, a token
    // indirection) empties both arrays, and `.every()` over [] is trivially true.
    const alphas = (source: string) =>
      [...source.matchAll(/var\(--accent\)\s+(\d+)%/g)].map(m => Number(m[1]));
    const restingShadow = declaration(stuck, 'box-shadow');
    const resting = alphas(restingShadow ?? '');
    const surgeStop = keyframes?.match(/\n\s*55%\s*\{([^}]*)\}/)?.[1] ?? '';
    const surge = alphas(surgeStop);
    expect(resting.length).toBe(3);
    expect(surge.length).toBe(resting.length);
    expect(surge.every((a, i) => a > resting[i])).toBe(true);

    // Every stop must match the base rule's layer count AND its inset-ness, or the
    // layers cannot interpolate: box-shadow list interpolation pads the shorter list
    // with a non-inset shadow, the per-layer inset flags then mismatch, and the whole
    // animation silently goes DISCRETE (the marker pops on instead of ramping). The
    // CSS comment claims this holds at every keyframe, so pin it at every keyframe.
    const insets = (shadow: string | undefined) =>
      layers(shadow).map(l => l.includes('inset')).join(',');
    for (const [name, stop] of [
      ['0%', zeroStop],
      ['55%', surgeStop],
    ] as const) {
      const stopShadow = declaration(stop, 'box-shadow');
      expect(layers(stopShadow).length, name).toBe(layers(restingShadow).length);
      expect(insets(stopShadow), name).toBe(insets(restingShadow));
    }
  });

  it('dissolves via an ANIMATION so it still runs when it interrupts the turn-on', () => {
    const fading = block('.nav-focus-stuck.nav-focus-fading');
    expect(fading).not.toBe('');

    // The dismiss must be an animation, not a transition. This is correctness, not
    // taste: while the turn-on animation runs it owns box-shadow, so a transition
    // underneath never starts, and cancelling the turn-on to free the property just
    // lands the value on the after-change style in the same frame with nothing to
    // interpolate. Measured on the real rule, that shape made a dismiss inside the
    // 0.5s ramp BLINK the marker off within two frames in Chromium. Replacing the
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
    // Deliberately SLOW, and much slower than the turn-on: the marker is being retired
    // because the user moved on, not cleared out of their way. Pin the relationship
    // rather than the number, so either duration can be retuned and keep it.
    const onSeconds = Number(
      declaration(block('.nav-focus-stuck'), 'animation')?.match(/([\d.]+)s/)?.[1],
    );
    expect(seconds).toBeGreaterThanOrEqual(onSeconds * 2);
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
