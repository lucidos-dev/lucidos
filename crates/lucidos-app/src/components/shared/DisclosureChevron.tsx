import { ChevronRightIcon } from './icons';

export function DisclosureChevron({ open }: { open: boolean }) {
  return (
    <span class="disclosure-chevron" data-open={open ? 'true' : 'false'} aria-hidden="true">
      <ChevronRightIcon />
    </span>
  );
}
