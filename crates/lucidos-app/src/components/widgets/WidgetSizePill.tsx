import { WIDGET_SIZES } from '@lucidos/engine-constants';
import type { WidgetSize } from '../../store/types';

const SIZE_LETTERS: Record<WidgetSize, string> = { small: 'S', medium: 'M', large: 'L', xl: 'XL' };

export function offeredSizes(sizes: readonly WidgetSize[]): WidgetSize[] {
  const offered = WIDGET_SIZES.filter((s) => sizes.includes(s));
  return offered.length < 2 ? [] : offered;
}

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
