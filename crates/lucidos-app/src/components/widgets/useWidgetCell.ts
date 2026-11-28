import { useLayoutEffect } from 'preact/hooks';
import { WIDGET_GAP_CSS, widgetCellCss } from '../../utils/dashboardGeometry';

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
