import type { JSX } from 'preact';

export type ToggleTone = 'framed' | 'glyph' | 'armed' | 'quiet' | 'live';

type ToggleIconButtonProps = Omit<JSX.ButtonHTMLAttributes<HTMLButtonElement>, 'class' | 'className' | 'type' | 'aria-pressed'> & {
  pressed: boolean;
  tone?: ToggleTone;
  class?: string;
};

export function ToggleIconButton({ pressed, tone = 'framed', class: extraClass, ...rest }: ToggleIconButtonProps) {
  return (
    <button
      type="button"
      {...rest}
      class={`icon-btn${extraClass ? ` ${extraClass}` : ''}`}
      data-tone={tone}
      aria-pressed={pressed}
    />
  );
}
