import { externalWebUrl } from '@lucidos/web-link';
import { openUrl } from './actions/artifacts';
import { openWorkspaceThreadUrl, parseWorkspaceThreadUrl } from './actions/cross-workspace';

export function routeExternalAnchorClick(e: MouseEvent): void {
  if (e.defaultPrevented) return;
  const anchor = (e.target as Element | null)?.closest?.('a[href]');
  const href = anchor?.getAttribute('href');
  if (!href) return;
  const url = externalWebUrl(href);
  if (!url) return;
  e.preventDefault();
  const threadUrl = parseWorkspaceThreadUrl(url);
  if (threadUrl) openWorkspaceThreadUrl(threadUrl);
  else openUrl(url);
}
