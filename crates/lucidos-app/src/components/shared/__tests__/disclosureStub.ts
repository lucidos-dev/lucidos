/**
 * A hook-free `<Disclosure>` for suites that flatten vnodes with a bare call
 * (`vnodeToText`), which cannot run hooks. It only shows or hides; the roll
 * itself is pinned in `disclosure.test.tsx`. Use it as:
 *
 *   vi.mock('<path>/shared/Disclosure', () => import('<path>/disclosureStub'));
 */
import type { ComponentChildren } from 'preact';

export function Disclosure({ open, children }: { open: boolean; children: ComponentChildren }) {
  return open ? children : null;
}
