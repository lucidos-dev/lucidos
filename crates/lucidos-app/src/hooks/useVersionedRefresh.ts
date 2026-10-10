import { useEffect, useRef } from 'preact/hooks';

/** Re-run `onChange` whenever `version` moves, and never on mount.
 *
 *  This is how a settings page that fetches its own data hears about an SSE
 *  frame. The dispatchers bump a version signal, the page passes it here, and
 *  the page re-reads. A page whose data lives in a store signal needs none of
 *  this: it subscribes by reading the signal (ADR 0118).
 *
 *  `paused` holds the refresh off while the page has a write in flight. That
 *  write re-reads when it settles, and a reply landing under it would revert
 *  the control the user just moved. The pause defers rather than drops: a
 *  frame that arrived during a write lands as soon as `paused` clears.
 *
 *  `onChange` is read through a ref, so a caller may pass a fresh closure each
 *  render without re-arming anything.
 */
export function useVersionedRefresh(
  version: number,
  paused: boolean,
  onChange: () => void,
): void {
  const latest = useRef(onChange);
  latest.current = onChange;
  const applied = useRef(version);

  useEffect(() => {
    if (paused || version === applied.current) return;
    applied.current = version;
    latest.current();
  }, [version, paused]);
}
