import { apiUrl, dataMountUrl, getBaseUrl, request, requestText, requestVoid } from './_fetch';
import { assertArray } from './_validate';
import { capabilityCarrier } from './frameCapability';
import { DATA_MUTATION_RESPONSE_DEADLINE_MS } from './generated/engine-constants';
import { parseAppId } from './scroll';

export interface WriteResult {
  success: boolean;
  commit?: string;
}

export interface UploadResult {
  success: boolean;
  filename?: string;
  error?: string;
}

export interface EditOperation {
  json_path?: string;
  json_value?: unknown;
  find?: string;
  replace?: string;
}

export const data = {
  read(path: string): Promise<string> {
    return requestText(`/data/${encodePathSegments(path)}`);
  },

  write(path: string, content: string): Promise<WriteResult> {
    return request('/data/' + encodePathSegments(path), {
      method: 'PUT',
      headers: { 'Content-Type': 'text/plain' },
      body: content,
    }, DATA_MUTATION_RESPONSE_DEADLINE_MS);
  },

  delete(path: string): Promise<void> {
    return requestVoid('/data/' + encodePathSegments(path), {
      method: 'DELETE',
    }, DATA_MUTATION_RESPONSE_DEADLINE_MS);
  },

  list(pattern?: string): Promise<string[]> {
    const qs = pattern ? `?pattern=${encodeURIComponent(pattern)}` : '';
    return request(`/data${qs}`);
  },

  url(path: string): string {
    if (path.startsWith('system-knowhow/')) {
      return apiUrl(`/data/${encodePathSegments(path)}`);
    }
    const carrier = capabilityCarrier();
    if (typeof window !== 'undefined') {
      const appUrl = buildAppLocalUrl(
        path,
        window.location.pathname,
        window.location.search,
        getBaseUrl(),
        carrier,
      );
      if (appUrl !== null) return appUrl;
    }
    return dataMountUrl(encodePathSegments(path));
  },

  edit(path: string, operations: EditOperation[]): Promise<void> {
    assertArray('operations', operations);
    return requestVoid('/data/edit', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path, operations }),
    });
  },

  upload(file: File): Promise<UploadResult> {
    const formData = new FormData();
    formData.append('file', file);
    return request('/data/upload', {
      method: 'POST',
      body: formData,
    }, 120000);
  },
};

function encodePathSegments(path: string): string {
  return path.split('/').map(encodeURIComponent).join('/');
}

export function buildAppLocalUrl(
  path: string,
  pathname: string,
  search: string,
  baseUrl: string,
  carrier = '',
): string | null {
  const appId = parseAppId(pathname, baseUrl);
  if (!appId) return null;
  const prefix = `apps/${appId}/`;
  if (!path.startsWith(prefix)) return null;
  const rest = path.slice(prefix.length);
  if (!rest) return null;

  const params = new URLSearchParams(search);
  const carry = new URLSearchParams();
  const threadId = params.get('thread_id');
  if (threadId) carry.set('thread_id', threadId);
  const qs = carry.toString();

  const encodedAppId = encodeURIComponent(appId);
  const encodedRest = encodePathSegments(rest);
  return `${baseUrl}${carrier}/app/${encodedAppId}/${encodedRest}${qs ? `?${qs}` : ''}`;
}
