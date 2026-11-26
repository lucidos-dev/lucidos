import { useId } from 'preact/hooks';
import { LucidosMarkIcon } from './icons';


const MARK_SCALE = 0.78;
const TILE_RADIUS = 22;
const GRAD = { cx: 30, cy: 22, r: 125, from: '#2d83e0', to: '#0a4ea8' };
const GLYPH_FILL = '#ffffff';
const OFFSET = (100 - 100 * MARK_SCALE) / 2;
const SPARK_D =
  'M68.5 12 C71 25 74 28.5 87 31 C74 33.5 71 37 68.5 50 ' +
  'C66 37 63 33.5 50 31 C63 28.5 66 25 68.5 12 Z';

export interface LucidosMarkProps {
  size?: string;
  background?: boolean;
  title?: string;
  class?: string;
}

export function LucidosMark({
  size = '1.25rem',
  background = true,
  title,
  class: className,
}: LucidosMarkProps) {
  const gradId = `lucidos-mark-grad-${useId()}`;
  const a11y = title
    ? { role: 'img' as const, 'aria-label': title }
    : { 'aria-hidden': true as const };
  const cls = 'lucidos-mark' + (className ? ` ${className}` : '');
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
          <stop offset="0" stop-color={`var(--lucidos-mark-bg-top, ${GRAD.from})`} />
          <stop offset="1" stop-color={`var(--lucidos-mark-bg-bottom, ${GRAD.to})`} />
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
      <g
        transform={`translate(${OFFSET} ${OFFSET}) scale(${MARK_SCALE})`}
        fill={`var(--lucidos-mark-fg, ${GLYPH_FILL})`}
      >
        <rect class="lmk-tile lmk-tile-1" x="17" y="17" width="29" height="29" rx="7" />
        <rect class="lmk-tile lmk-tile-2" x="17" y="54" width="29" height="29" rx="7" />
        <rect class="lmk-tile lmk-tile-3" x="54" y="54" width="29" height="29" rx="7" />
        <path class="lmk-spark" d={SPARK_D} />
      </g>
    </svg>
  );
}

export function LucidosGlyph({ size = 'var(--icon-size-sm)' }: { size?: string }) {
  return (
    <>
      <LucidosMarkIcon class="lucidos-agent-flat" size={size} />
      <LucidosMark size={size} class="lucidos-agent-tile" />
    </>
  );
}
