
import { API } from '../../utils/basePath';

export function repoFileUrl(repoId: string, path: string, gitRef?: string): string {
  const params = new URLSearchParams({ path });
  if (gitRef) params.set('ref', gitRef);
  return `${API}/repositories/${encodeURIComponent(repoId)}/file?${params}`;
}

export function changeFileUrl(changeId: string, path: string): string {
  const params = new URLSearchParams({ path });
  return `${API}/changes/${encodeURIComponent(changeId)}/file?${params}`;
}
