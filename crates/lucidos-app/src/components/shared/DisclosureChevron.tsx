import { ChevronRightIcon } from './icons';

/** The marker beside a fold's toggle: one chevron in a fixed box, turned a
 *  quarter turn when the fold is open.
 *
 *  A rotated icon keeps one footprint in both states, so the label beside it
 *  never shifts as it toggles. The box is `--disclosure-chevron-size`, set by
 *  the surface (`.disclosure-chevron` in steps.css). */
export function DisclosureChevron({ open }: { open: boolean }) {
  return (
    <span class="disclosure-chevron" data-open={open ? 'true' : 'false'} aria-hidden="true">
      <ChevronRightIcon />
    </span>
  );
}
