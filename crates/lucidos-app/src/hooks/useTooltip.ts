/**
 * Global tooltip for the host shell.
 *
 * The behaviour lives in `@lucidos/tooltip`, which app iframes run too, so the
 * host and every app share one implementation and one set of CSS rules. Mount
 * this hook once near the app root. Any element with `data-tooltip` is then
 * covered, including one added later.
 *
 * Host-only choices, both defaults of the shared module:
 *   - a touch long press reveals only for `data-tooltip-longpress` elements, so
 *     a plain tappable row keeps its tap
 *   - a revealed tooltip stays up until the next tap
 */
import { useEffect } from 'preact/hooks';
import { installTooltips } from '@lucidos/tooltip';

export function useTooltip() {
  useEffect(() => installTooltips(), []);
}
