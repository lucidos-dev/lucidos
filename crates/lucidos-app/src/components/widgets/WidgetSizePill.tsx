import { WIDGET_SIZES } from '@lucidos/engine-constants';
import type { WidgetSize } from '../../store/types';

/** The size pill's letter for each size. */
const SIZE_LETTERS: Record<WidgetSize, string> = { small: 'S', medium: 'M', large: 'L', xl: 'XL' };

/** The sizes a pill offers for a widget declaring `sizes`, in size order. One
 *  size is no choice, so it offers none. */
export function offeredSizes(sizes: readonly WidgetSize[]): WidgetSize[] {
  const offered = WIDGET_SIZES.filter((s) => sizes.includes(s));
  return offered.length < 2 ? [] : offered;
}

/** The S · M · L · XL pill a thread card's widget menu and an edited dashboard
 *  tile draw. Only the sizes the widget declares are offered. */
export function WidgetSizePill({ name, sizes, current, onPick }: {
  name: string;
  sizes: readonly WidgetSize[];
  current: WidgetSize;
  onPick: (size: WidgetSize) => void;
}) {
  const offered = offeredSizes(sizes);
  if (offered.length === 0) return null;
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
