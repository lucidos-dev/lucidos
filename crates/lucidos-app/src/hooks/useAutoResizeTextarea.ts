import { useEffect, useRef } from 'preact/hooks';
import { autoResizeTextarea } from '../utils/dom';

/** A textarea ref that keeps the field exactly as tall as its text.
 *
 *  It re-measures when `value` changes and when the field's width changes,
 *  since a narrower pane wraps the same text onto more lines. Only a width
 *  change re-measures, because the measure itself sets the height. */
export function useAutoResizeTextarea(value: string) {
  const ref = useRef<HTMLTextAreaElement>(null);

  useEffect(() => autoResizeTextarea(ref.current), [value]);

  useEffect(() => {
    const el = ref.current;
    if (!el || typeof ResizeObserver === 'undefined') return;
    let width = el.offsetWidth;
    const observer = new ResizeObserver(() => {
      if (el.offsetWidth === width) return;
      width = el.offsetWidth;
      autoResizeTextarea(el);
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  return ref;
}
