import { apiUrl, getBaseUrl, request, requestText, requestVoid } from './_fetch';
import { assertArray } from './_validate';
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
    return requestText(`/data/${encodePathSegments(path)}`);
  },

  write(path: string, content: string): Promise<WriteResult> {
    return request('/data/' + encodePathSegments(path), {
      method: 'PUT',
      headers: { 'Content-Type': 'text/plain' },
      body: content,
    });
  },

  delete(path: string): Promise<void> {
    return requestVoid('/data/' + encodePathSegments(path), {
      method: 'DELETE',
    });
  },

  list(pattern?: string): Promise<string[]> {
    const qs = pattern ? `?pattern=${encodeURIComponent(pattern)}` : '';
    return request(`/data${qs}`);
  },

  url(path: string): string {
    // system-knowhow lives in the engine repo, not the workspace, so it isn't
    // served by the static `/data` mount. Route it through the API endpoint
    // which dispatches to the engine's system_knowhow_dir.
    if (path.startsWith('system-knowhow/')) {
      return apiUrl(`/data/${encodePathSegments(path)}`);
    }
    // When the SDK runs inside an app iframe (`/app/<id>/...`) and the caller
    // asks for the app's *own* bundled asset (`apps/<id>/foo.png`), route
    // through `/app/<id>/foo.png?<carried>` instead of `/data/...`. The
    // `/app/:app_id/*path` route honours `?thread_id=` (WIP preview from a
    // coding-agent worktree) — the static `/data/` mount does not, so a JS-set
    // src would 404 in WIP-preview even when the asset exists on the agent's
    // branch. The engine's HTML rewriter (api/app_ui.rs) handles markup
    // `<img src="…">` but skips `<script>` bodies, leaving JS-set src as the
    // remaining footgun this closes.
    if (typeof window !== 'undefined') {
      const appUrl = buildAppLocalUrl(
        path,
        window.location.pathname,
        window.location.search,
        getBaseUrl(),
      );
      if (appUrl !== null) return appUrl;
    }
    return `${getBaseUrl()}/data/${encodePathSegments(path)}`;
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

/** Encode each segment of a path individually, preserving `/` separators. */
function encodePathSegments(path: string): string {
  return path.split('/').map(encodeURIComponent).join('/');
}

/**
 * If the SDK is loaded inside `/app/<currentAppId>/...` and `path` refers to
 * an asset under the same app's folder (`apps/<currentAppId>/<rest>`), build
 * the equivalent `/app/<currentAppId>/<rest>?<carried>` URL — carrying over
 * `?thread_id=` from the iframe so the asset request lands on the same WIP /
 * live branch as the iframe itself. Returns `null` for every other case so the
 * caller falls through to the default `/data/...` mount (which serves from the
 * live workspace).
 *
 * Exported for unit testing — the production call site reads `pathname` /
 * `search` / `baseUrl` from `window` + `getBaseUrl()`.
 */
export function buildAppLocalUrl(
  path: string,
  pathname: string,
  search: string,
  baseUrl: string,
): string | null {
  const appId = parseAppId(pathname);
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
  return `${baseUrl}/app/${encodedAppId}/${encodedRest}${qs ? `?${qs}` : ''}`;
}
