import { useId } from 'preact/hooks';

// The Lucidos mark, as a reusable inline-SVG brand component.
//
// SINGLE SOURCE OF TRUTH for the geometry + gradient is the icon generator
// `crates/lucidos-app/scripts/generate-brand-icons.mjs` (which emits the
// applied favicon / app-icon set). The constants below are mirrored from it
// verbatim — if you tweak the brand there, mirror the change here (and vice
// versa). Do NOT invent new values.
//
//   - mark: three rounded tiles forming an "L" + a 4-point spark (the missing
//     4th tile), authored in a 0..100 grid, solid white.
//   - gradient: radial azure→cobalt, light source upper-left. CSS equivalent
//     `radial-gradient(125% 125% at 30% 22%, #2d83e0 0%, #0a4ea8 100%)`.
//   - the mark fills ~78% of the square canvas, centered (SCALE_FULLBLEED — the
//     unmasked / lightly-masked regime; this inline glyph is never platform-
//     masked, so it tracks the full-bleed app-icon scale, not the maskable one);
//     behind it a rounded-rect tile (22% corner radius) carries the gradient
//     edge-to-edge.

const MARK_SCALE = 0.78; // SCALE_FULLBLEED — mark fraction of the canvas
const TILE_RADIUS = 22; // FAVICON_SVG_RADIUS * 100 — rounded-rect bg corner
const GRAD = { cx: 30, cy: 22, r: 125, from: '#2d83e0', to: '#0a4ea8' };
// translate offset that centers the scaled mark grid inside the 0..100 viewBox.
const OFFSET = (100 - 100 * MARK_SCALE) / 2; // 11
// Spark path (cubic beziers), verbatim from the generator's SPARK_D.
const SPARK_D =
  'M68.5 12 C71 25 74 28.5 87 31 C74 33.5 71 37 68.5 50 ' +
  'C66 37 63 33.5 50 31 C63 28.5 66 25 68.5 12 Z';

export interface LucidosMarkProps {
  /** CSS length for the (square) mark. Default `1.25rem`. */
  size?: string;
  /** Play the reveal animation once on mount, then land on the static mark.
   *  Default `false`. Honors `prefers-reduced-motion` (renders static). */
  animated?: boolean;
  /** Render the gradient rounded-rect tile behind the mark (the standalone
   *  glyph / tile look). Default `true`. Set `false` to render only the white
   *  mark shapes on a transparent background — e.g. on the boot splash, where
   *  the backdrop already carries the gradient. */
  background?: boolean;
  /** Accessible label. When set, the mark is exposed as `role="img"` with this
   *  label; otherwise it is decorative (`aria-hidden`). */
  title?: string;
  /** Extra class on the root `<svg>`. */
  class?: string;
}

export function LucidosMark({
  size = '1.25rem',
  animated = false,
  background = true,
  title,
  class: className,
}: LucidosMarkProps) {
  // Unique per instance: many marks can render at once (one per chat exchange),
  // and `url(#id)` resolves to the first matching id in document order — so a
  // shared id would break the gradient fill the moment that first instance
  // unmounts (the exchange scrolls out of the DOM).
  const gradId = `lucidos-mark-grad-${useId()}`;
  const a11y = title
    ? { role: 'img' as const, 'aria-label': title }
    : { 'aria-hidden': true as const };
  const cls =
    'lucidos-mark' +
    (animated ? ' lucidos-mark-animated' : '') +
    (className ? ` ${className}` : '');
  // Sized via inline style only (not SVG width/height attributes): `size` may
  // be a CSS expression — var(), min() — that isn't valid as an SVG attribute.
  return (
    <svg
      class={cls}
      viewBox="0 0 100 100"
      style={{ width: size, height: size }}
      {...a11y}
    >
      <defs>
        <radialGradient
          id={gradId}
          gradientUnits="userSpaceOnUse"
          cx={GRAD.cx}
          cy={GRAD.cy}
          r={GRAD.r}
        >
          <stop offset="0" stop-color={GRAD.from} />
          <stop offset="1" stop-color={GRAD.to} />
        </radialGradient>
      </defs>
      {background && (
        <rect
          class="lmk-bg"
          x="0"
          y="0"
          width="100"
          height="100"
          rx={TILE_RADIUS}
          fill={`url(#${gradId})`}
        />
      )}
      <g transform={`translate(${OFFSET} ${OFFSET}) scale(${MARK_SCALE})`} fill="#ffffff">
        <rect class="lmk-tile lmk-tile-1" x="17" y="17" width="29" height="29" rx="7" />
        <rect class="lmk-tile lmk-tile-2" x="17" y="54" width="29" height="29" rx="7" />
        <rect class="lmk-tile lmk-tile-3" x="54" y="54" width="29" height="29" rx="7" />
        <path class="lmk-spark" d={SPARK_D} />
      </g>
    </svg>
  );
}

/** The Lucidos Agent brand glyph — the mark at the chat actor-chip icon size.
 *  Single source for the "Lucidos Agent" icon across the executor/initiator
 *  chips and the control menu (replaces the former ✨ placeholder). */
export function LucidosAgentGlyph({ size = 'var(--icon-size-sm)' }: { size?: string }) {
  return <LucidosMark size={size} />;
}
