import { useEffect, useRef } from 'preact/hooks';
import { autoResizeTextarea } from '../utils/dom';

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
