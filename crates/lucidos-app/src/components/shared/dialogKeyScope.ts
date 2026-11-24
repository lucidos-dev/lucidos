import { topOverlay } from '../../store/overlayStack';

export function dialogOwnsKey(target: EventTarget | null, panel: HTMLElement | null): boolean {
  const el = target as { closest?: (selector: string) => Element | null } | null;
  const owner = typeof el?.closest === 'function' ? el.closest('[data-overlay-panel]') : null;
  if (owner !== null) return owner === panel;
  return panelIsTopOverlay(panel);
}

function panelIsTopOverlay(panel: HTMLElement | null): boolean {
  const el = panel as { getAttribute?: (name: string) => string | null } | null;
  const ownId = typeof el?.getAttribute === 'function' ? el.getAttribute('data-overlay-panel') : null;
  if (!ownId) return true;
  const top = topOverlay();
  return top === null || top.id === ownId;
}
