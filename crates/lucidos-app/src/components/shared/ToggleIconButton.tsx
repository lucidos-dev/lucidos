import type { JSX } from 'preact';

/** How a pressed toggle icon button looks. The set is closed. Each tone is
 *  painted once per surface: `styles/global/host-components.css` for the page,
 *  `styles/panels/shell.css` for the header bar. A new look is a new tone here,
 *  never a local rule.
 *
 *  - `framed`: accent glyph in a chip. The default, and what Filter wears.
 *  - `glyph`: accent glyph, no chip. A standing mode (Pin, Follow, Fold).
 *  - `armed`: green glyph, no chip. A standing Apply, armed for the thread.
 *  - `quiet`: neutral chip, no accent. Controls that repeat on every turn.
 *  - `live`: green glyph in a green chip. A call that is up. */
export type ToggleTone = 'framed' | 'glyph' | 'armed' | 'quiet' | 'live';

type ToggleIconButtonProps = Omit<JSX.ButtonHTMLAttributes<HTMLButtonElement>, 'class' | 'className' | 'type' | 'aria-pressed'> & {
  /** The on-state. It is `aria-pressed`, the one hook every tone paints from. */
  pressed: boolean;
  tone?: ToggleTone;
  /** The size band and any hook class: `header-icon`, `pin-btn`. Never a paint. */
  class?: string;
};

/** The one icon button that toggles. It renders `aria-pressed` and a tone. The
 *  surface it sits on (page or header bar) picks the paint, so a toggle cannot
 *  wear the wrong one. Every icon toggle renders through here, or through
 *  `renderHeaderAction`, which does (`toggle-icon-single-source.test.ts`). */
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
