import type { ComponentChildren } from 'preact';

export type CrossfadeLayer = { key: string; node: ComponentChildren };

/** Crossfades between a fixed set of layers stacked in one grid cell
 *  (`.crossfade-stack`, global/host-components.css).
 *
 *  Every layer stays mounted and only `current` is opaque. So a swap is an
 *  opacity transition on elements that already exist. It reverses from
 *  wherever it is, and it starts in the same frame as its neighbours: WebKit
 *  can start an animation on a freshly mounted element a frame late.
 *
 *  The cell is as big as the biggest layer, so a swap never changes the box.
 *  Hidden layers are `aria-hidden`, so a screen reader reads only the current
 *  one. Hook-free, so a test can call it directly. */
export function CrossfadeStack({ layers, current, class: className }: {
  layers: readonly CrossfadeLayer[];
  current: string;
  class?: string;
}) {
  return (
    <span class={className ? `crossfade-stack ${className}` : 'crossfade-stack'}>
      {layers.map(({ key, node }) => (
        <span
          key={key}
          class="crossfade-layer"
          data-layer={key}
          data-current={key === current ? '' : undefined}
          aria-hidden={key === current ? undefined : 'true'}
        >
          {node}
        </span>
      ))}
    </span>
  );
}
