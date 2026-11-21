import { externalWebUrl } from '@lucidos/web-link';
import { openUrl } from './actions/artifacts';
import { openWorkspaceThreadUrl, parseWorkspaceThreadUrl } from './actions/cross-workspace';

/** Route a click on a web link that no component handled, so the browser never
 *  navigates the shell to it itself. A navigation off Lucidos can push a history
 *  entry that the iOS back gesture then lands on, outside the app.
 *
 *  The scheme's case and a protocol-relative `//host` make no difference
 *  (`externalWebUrl`). A click an earlier handler already cancelled is theirs. */
export function routeExternalAnchorClick(e: MouseEvent): void {
  if (e.defaultPrevented) return;
  const anchor = (e.target as Element | null)?.closest?.('a[href]');
  const href = anchor?.getAttribute('href');
  if (!href) return;
  const url = externalWebUrl(href);
  if (!url) return;
  e.preventDefault();
  // A pasted link to a Lucidos thread stays in Lucidos.
  const threadUrl = parseWorkspaceThreadUrl(url);
  if (threadUrl) openWorkspaceThreadUrl(threadUrl);
  else openUrl(url);  // Panel in Tauri, new tab in browser
}
