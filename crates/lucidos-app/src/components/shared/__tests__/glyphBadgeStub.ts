/**
 * A hook-free `<GlyphBadge>` for suites that flatten vnodes with a bare call,
 * which cannot run hooks. It draws the plain span; the ink measurement is
 * pinned in `GlyphBadge.test.tsx`. Use it as:
 *
 *   vi.mock('<path>/shared/GlyphBadge', () => import('<path>/glyphBadgeStub'));
 */
import { h, type JSX } from 'preact';

export function GlyphBadge(props: JSX.HTMLAttributes<HTMLSpanElement>) {
  return h('span', props);
}
