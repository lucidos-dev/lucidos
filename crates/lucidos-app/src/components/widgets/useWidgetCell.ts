import { useLayoutEffect } from 'preact/hooks';
import { WIDGET_GAP_CSS, widgetCellCss } from '../../utils/dashboardGeometry';

/** Set the cell edge and gap every *widget box* under `host` draws from, from
 *  `host`'s own width: a dashboard, or the thread pane a phone dashboard fills.
 *  Written straight to the style, so a resize re-renders nothing. */
export function useWidgetCell(host: HTMLElement | null): void {
  useLayoutEffect(() => {
    if (!host) return undefined;
    const measure = () => {
      host.style.setProperty('--widget-cell', widgetCellCss(host.clientWidth));
      host.style.setProperty('--widget-gap', WIDGET_GAP_CSS);
    };
    measure();
    if (typeof ResizeObserver !== 'function') return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(host);
    return () => observer.disconnect();
  }, [host]);
}
