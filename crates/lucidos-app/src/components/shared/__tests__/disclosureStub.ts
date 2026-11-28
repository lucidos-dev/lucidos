import type { ComponentChildren } from 'preact';

export function Disclosure({ open, children }: { open: boolean; children: ComponentChildren }) {
  return open ? children : null;
}
