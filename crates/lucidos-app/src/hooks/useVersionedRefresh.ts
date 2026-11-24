import { useEffect, useRef } from 'preact/hooks';

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
