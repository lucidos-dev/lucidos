// URL builders for a repository file's raw bytes. A leaf module, importing
// only `API`, so `utils/renderMarkdown` can point a repo document's images
// here without loading the store that `./_core` pulls in.

import { API } from '../../utils/basePath';

/** URL of a repo file's raw bytes at `gitRef` (default HEAD). The engine serves
 *  it with a content-type inferred from the extension, so this is safe to point
 *  an <img>/<video>/<audio>/<iframe> `src` at for media previews. An HTML, SVG
 *  or XML body comes sandboxed with script off, so a frame never runs one. */
export function repoFileUrl(repoId: string, path: string, gitRef?: string): string {
  const params = new URLSearchParams({ path });
  if (gitRef) params.set('ref', gitRef);
  return `${API}/repositories/${encodeURIComponent(repoId)}/file?${params}`;
}

/** URL of the full "after" version of a file in a change: at its branch while
 *  pending, at its post-merge sha once applied. Any path in the repository
 *  resolves, not only the files the change touched. Served with a content-type
 *  inferred from the extension, so a media preview can point an
 *  <img>/<video>/<audio>/<iframe> `src` at it. An HTML, SVG or XML body comes
 *  sandboxed with script off, as `repoFileUrl` does. */
export function changeFileUrl(changeId: string, path: string): string {
  const params = new URLSearchParams({ path });
  return `${API}/changes/${encodeURIComponent(changeId)}/file?${params}`;
}
