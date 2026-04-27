import { request, requestText, requestVoid, getBaseUrl } from './_fetch';
import { assertArray } from './_validate';

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
  /** JSON path edit */
  json_path?: string;
  json_value?: unknown;
  /** Text find-replace edit */
  find?: string;
  replace?: string;
}

export const data = {
  read(path: string): Promise<string> {
    return requestText(`/api/v1/data/${encodePathSegments(path)}`);
  },

  write(path: string, content: string): Promise<WriteResult> {
    return request('/api/v1/data/' + encodePathSegments(path), {
      method: 'PUT',
      headers: { 'Content-Type': 'text/plain' },
      body: content,
    });
  },

  delete(path: string): Promise<void> {
    return requestVoid('/api/v1/data/' + encodePathSegments(path), {
      method: 'DELETE',
    });
  },

  list(pattern?: string): Promise<string[]> {
    const qs = pattern ? `?pattern=${encodeURIComponent(pattern)}` : '';
    return request(`/api/v1/data${qs}`);
  },

  url(path: string): string {
    return `${getBaseUrl()}/data/${encodePathSegments(path)}`;
  },

  edit(path: string, operations: EditOperation[]): Promise<void> {
    assertArray('operations', operations);
    return requestVoid('/api/v1/data/edit', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path, operations }),
    });
  },

  upload(file: File): Promise<UploadResult> {
    const formData = new FormData();
    formData.append('file', file);
    return request('/api/v1/data/upload', {
      method: 'POST',
      body: formData,
    }, 120000);
  },
};

/** Encode each segment of a path individually, preserving `/` separators. */
function encodePathSegments(path: string): string {
  return path.split('/').map(encodeURIComponent).join('/');
}
