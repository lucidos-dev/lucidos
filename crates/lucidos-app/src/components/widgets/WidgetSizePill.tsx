import { WIDGET_SIZES } from '@lucidos/engine-constants';
import type { WidgetSize } from '../../store/types';

/** The size pill's letter for each size. */
const SIZE_LETTERS: Record<WidgetSize, string> = { small: 'S', medium: 'M', large: 'L' };

/** The S · M · L pill a thread card's bar and an edited dashboard tile draw.
 *  Only the sizes the widget declares are offered, and one size offers none. */
export function WidgetSizePill({ name, sizes, current, onPick }: {
  name: string;
  sizes: readonly WidgetSize[];
  current: WidgetSize;
  onPick: (size: WidgetSize) => void;
}) {
  const offered = WIDGET_SIZES.filter((s) => sizes.includes(s));
  if (offered.length < 2) return null;
  return (
    <div class="widget-size-pill" role="radiogroup" aria-label={`Size of ${name}`}>
      {offered.map((size) => (
        <button
          key={size}
          type="button"
          role="radio"
          aria-checked={current === size}
          aria-label={size}
          class={current === size ? 'is-current' : undefined}
          onClick={() => onPick(size)}
        >
          {SIZE_LETTERS[size]}
        </button>
      ))}
    </div>
  );
}
