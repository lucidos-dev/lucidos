import { useState, useEffect } from 'preact/hooks';
import { getActiveScrollElement, isElementOnScreen } from '../components/chat/scrollState';

export function useOnScreenInTranscript(el: HTMLElement | null): boolean {
  const [onScreen, setOnScreen] = useState(true);
  const root = getActiveScrollElement();
  useEffect(() => (el ? watchOnScreen(el, setOnScreen) : undefined), [el, root]);
  return onScreen;
}

export function watchOnScreen(el: HTMLElement, onChange: (onScreen: boolean) => void): () => void {
  const sample = () => onChange(isElementOnScreen(el));
  sample();
  if (typeof IntersectionObserver !== 'function') return () => {};
  const observer = new IntersectionObserver(sample, { root: getActiveScrollElement() });
  observer.observe(el);
  return () => observer.disconnect();
}
