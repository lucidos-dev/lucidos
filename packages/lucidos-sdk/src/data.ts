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
  /**
   * JSON path edit. Accepts dot-bracket notation with quoted keys for
   * non-identifier names. All resolve to RFC 6901 JSON Pointers:
   *
   *   `metadata.author.name`              → `/metadata/author/name`
   *   `sections[1].slides[0].title`       → `/sections/1/slides/0/title`
   *   `dailyLog["2026-05-04"]`            → `/dailyLog/2026-05-04`
   *   `dailyLog['2026-05-04']`            → `/dailyLog/2026-05-04`
   *   `$.streak` (JSONPath root marker)   → `/streak`
   *   `/sections/1/title` (raw pointer)   → `/sections/1/title`
   *
   * Use quoted keys for any key that isn't a bare identifier (dates,
   * slugs with dots, keys containing spaces, etc.). RFC 6901 escaping
   * (`~` → `~0`, `/` → `~1`) is applied automatically inside quoted keys.
   */
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
    // system-knowhow lives in the engine repo, not the workspace, so it isn't
    // served by the static `/data` mount. Route it through the API endpoint
    // which dispatches to the engine's system_knowhow_dir.
    if (path.startsWith('system-knowhow/')) {
      return `${getBaseUrl()}/api/v1/data/${encodePathSegments(path)}`;
    }
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
