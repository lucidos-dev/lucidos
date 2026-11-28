import type { ComponentChildren } from 'preact';

export type CrossfadeLayer = { key: string; node: ComponentChildren };

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
