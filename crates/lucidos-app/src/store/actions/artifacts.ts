import {
  artifacts,
  filePreviewRevision,
  expandedFolders,
  panelOverlay,
  webviewInitialUrl,
  filePreviewSource,
  selectedLines,
  lineScrollTarget,
  showToast,
  dismissToast,
  parseRepoPath,
} from '../store';
import { toFailed, setLoadingIfFresh } from '../types';
import { listArtifacts, uploadFile } from '../../api/client';
import { revealContentPane } from './pane';
import { pushNavState } from './navigation';
import { isTauri } from '../../utils/platform';
import { openExternalUrl } from '../../utils/openExternalUrl';
import { DATA_PATH_PREFIXES } from '../../utils/linkifyPaths';
import { openExternal } from '../../utils/tauri';
import { errorDetail } from '../../utils/errorDetail';
import { inAppBrowserAvailable } from './preferences';

// The file-preview restore is a page-reload re-hydration step — it belongs to
// the FIRST loadArtifacts() after a fresh load, never to the SSE-driven
// refreshes that fire all session long (artifact created/edited during an
// agent run → DataFileEdited → loadArtifacts). Without this one-shot gate, any
// such refresh re-opens the last-viewed file and yanks the content pane there,
// clobbering an open app/URL/form mid-conversation — e.g. "Refreshing Planer"
// jumping back to the last generated PDF. Resets to false on page reload
// (module re-init).
let filePreviewRestoreAttempted = false;

export async function loadArtifacts(): Promise<void> {
  setLoadingIfFresh(artifacts);
  try {
    const data = await listArtifacts();
    const paths = data.artifacts || [];
    artifacts.value = { status: 'loaded', data: paths };

    // Expand top-level folders by default on first load
    if (expandedFolders.value.size === 0 && paths.length > 0) {
      const tree = buildFolderTree(paths);
      const newExpanded = new Set<string>();
      for (const folderName of Object.keys(tree.children)) {
        newExpanded.add(folderName);
      }
      expandedFolders.value = newExpanded;
    }

    // Restore previously open file preview — once, on the first load after a
    // page reload (see filePreviewRestoreAttempted above).
    if (!filePreviewRestoreAttempted) {
      filePreviewRestoreAttempted = true;
      const savedPath = localStorage.getItem('file-preview-open');
      if (savedPath && panelOverlay.value?.type !== 'file-preview') {
        if (paths.includes(savedPath)) {
          openFilePreview(savedPath, { preserveSource: true });
        } else {
          localStorage.removeItem('file-preview-open');
        }
      }
    }
  } catch (error) {
    artifacts.value = toFailed(error);
  }
}

export function toggleFolder(path: string): void {
  const newSet = new Set(expandedFolders.value);
  if (newSet.has(path)) {
    newSet.delete(path);
  } else {
    newSet.add(path);
  }
  expandedFolders.value = newSet;
}

const UPLOAD_TOAST_KEY = 'upload-progress';

export async function uploadFiles(files: FileList | File[]): Promise<void> {
  const fileList = Array.from(files);
  const total = fileList.length;
  let succeeded = 0;
  let failed = 0;
  const errors: string[] = [];

  for (let i = 0; i < fileList.length; i++) {
    const file = fileList[i];
    const label = total === 1
      ? `Importing ${file.name}`
      : `Importing ${i + 1}/${total}: ${file.name}`;
    showToast(label, 'info', { key: UPLOAD_TOAST_KEY, spinning: true });
    try {
      const data = await uploadFile(file);
      if (data.success) {
        succeeded++;
      } else {
        failed++;
        errors.push(`${file.name}: ${data.error || 'Unknown error'}`);
      }
    } catch (error) {
      failed++;
      errors.push(`${file.name}: ${errorDetail(error)}`);
    }
  }

  if (failed === 0) {
    dismissToast(UPLOAD_TOAST_KEY);
    const msg = succeeded === 1 ? '1 file imported' : `${succeeded} files imported`;
    showToast(msg, 'success');
  } else {
    const summary = succeeded > 0
      ? `${succeeded} imported, ${failed} failed`
      : failed === 1 ? 'Import failed' : `${failed} imports failed`;
    showToast(`${summary} — ${errors.join('; ')}`, 'error', { key: UPLOAD_TOAST_KEY });
  }

  await loadArtifacts();
}

// --- Path normalization ---

/** Ensure a data path starts with a known directory prefix.
 *  The navigate_ui tool may receive paths without the prefix — normalize
 *  to match the format expected by the /data/* static mount.
 *
 *  A repo-encoded preview path (`repo:<repoId>:file:<path>`, built by
 *  `encodeRepoPath`) is NOT a data path: it addresses a file inside a
 *  registered local repo clone, served by `/api/v1/repositories/:id/file`,
 *  and `ContentPane` routes it to `RepoFilePreview` instead of the /data/*
 *  mount. Prefixing it with `artifacts/` would make `parseRepoPath` reject it
 *  and dead-end the preview, so it passes through untouched — that's what lets
 *  an app iframe open a repo file via `lucidos.ui.navigate('file', …)`.
 *  Keyed off the same parser ContentPane routes on (not a bare `repo:` test),
 *  so a malformed `repo:` string still normalizes like any other data path. */
export function normalizeDataPath(path: string): string {
  if (parseRepoPath(path)) return path;
  if (DATA_PATH_PREFIXES.some(p => path.startsWith(p))) return path;
  return `artifacts/${path}`;
}

// --- File preview window actions ---

export function openFilePreview(path: string, opts?: { preserveSource?: boolean }): void {
  if (!opts?.preserveSource) filePreviewSource.value = false;
  // Drop any line selection and pending scroll from the file being replaced.
  // Both previews render `selectedLines`, so without this a range picked in one
  // file would highlight whatever rows happen to sit at those numbers in the
  // next one, and a scroll request that never found its file (a load error, a
  // format with no source view) would fire on an unrelated file later.
  // `openRepoFilePreview` clears the same pair on its own path, which sets
  // panelOverlay directly for its push-vs-replace logic. The navigate-to-a-line
  // router deliberately sets its range AFTER calling in here.
  selectedLines.value = null;
  lineScrollTarget.value = null;
  panelOverlay.value = { type: 'file-preview', path };
  localStorage.setItem('file-preview-open', path);
  revealContentPane();
  pushNavState();
}

/** Coalesce a flurry of announcements into one reload, the same 150 ms
 *  `refreshAppUI` uses on the app iframe. An `artifacts/` write announces
 *  twice, once as its `Artifact*` event and once as the file tool's
 *  `ToolResult`. A multi-file edit announces once per file.
 *
 *  One module-scoped slot is enough. Only one file preview is open at a time,
 *  and `invalidateFilePreview` drops every other path, so two pending bumps
 *  always name the same file. */
let previewBumpDebounce: ReturnType<typeof setTimeout> | null = null;
const PREVIEW_BUMP_DEBOUNCE_MS = 150;

function bumpOpenPreview(path: string): void {
  if (previewBumpDebounce) clearTimeout(previewBumpDebounce);
  previewBumpDebounce = setTimeout(() => {
    previewBumpDebounce = null;
    const current = filePreviewRevision.peek();
    filePreviewRevision.value = {
      path,
      rev: current?.path === path ? current.rev + 1 : 1,
    };
  }, PREVIEW_BUMP_DEBOUNCE_MS);
}

/** Re-fetch the open file preview, but ONLY when `path` is the file it shows.
 *
 *  This is the whole reason the revision carries a path. Every write under
 *  `data/` used to re-URL whatever was on screen, so an agent editing an
 *  unrelated file restarted a video the user was watching. The app iframe has
 *  scoped its own refresh on `app_id` since it grew one (`refreshAppUI`).
 *
 *  `path` is data-relative (`artifacts/clip.mp4`), matching what `panelOverlay`
 *  carries. An `Artifact*` event's `artifact_path` is relative to `artifacts/`
 *  and must be prefixed by the caller.
 *
 *  A write to a file the preview is NOT showing is dropped rather than banked.
 *  Coming back to that file remounts the element on the same URL. The engine
 *  sends `Cache-Control: no-cache` on every response (`api/mod.rs`), so the
 *  browser revalidates and cannot serve the pre-write bytes. */
export function invalidateFilePreview(path: string): void {
  const overlay = panelOverlay.peek();
  if (overlay?.type !== 'file-preview' || overlay.path !== path) return;
  bumpOpenPreview(path);
}

/** Re-fetch whatever the preview shows, because the user asked: the header
 *  Refresh button, and the inline editor once a save has landed. */
export function refreshFilePreview(): void {
  const overlay = panelOverlay.peek();
  if (overlay?.type !== 'file-preview') return;
  bumpOpenPreview(overlay.path);
}

// --- URL preview in panel ---

/** Normalize a URL to match Rust's url::Url normalization (trailing slash, lowercase, etc.)
 *  so that URL comparisons between frontend and backend are consistent. */
export function normalizeUrl(url: string): string {
  try { return new URL(url).href; } catch { return url; }
}

/** Open a URL AWAY from the app: a browser tab, or the OS default browser under
 *  Tauri. Never the in-app url-preview panel, whatever the toggle says.
 *
 *  `openUrl` delegates here whenever the in-app browser is not the live target,
 *  so each branch has one copy. Call it directly when the destination must not
 *  be embedded at all. An OAuth authorization page is the case that named it:
 *  providers refuse a sign-in flow inside an embedded webview.
 *
 *  `source` says where a navigate the user did NOT click came from (a thread
 *  label, or "an app"). A "couldn't open it" toast then names what asked,
 *  instead of appearing out of nowhere. Same shape as `openAppById`'s. */
export function openUrlOutsideApp(url: string, source?: string): void {
  const normalized = normalizeUrl(url);
  if (!isTauri()) {
    // Browser + PWA: a new tab, except on an installed iOS PWA where that would
    // be the inescapable in-app web view. See utils/openExternalUrl.ts, which
    // owns the toast for a tab the browser refused to open.
    openExternalUrl(normalized, source);
    return;
  }
  // The OS opener, the same path openLocalFile uses.
  const from = source ? ` (requested by ${source})` : '';
  void openExternal(normalized).catch((err) =>
    showToast(`Couldn't open ${normalized}${from}: ${errorDetail(err)}`, 'error'),
  );
}

/** Open a URL wherever the user's preferences point it. The experimental in-app
 *  browser is opt-in and desktop-only, so it mounts the url-preview panel only
 *  when it is the live target. Everything else goes to `openUrlOutsideApp`. */
export function openUrl(url: string, source?: string): void {
  if (!inAppBrowserAvailable()) {
    openUrlOutsideApp(url, source);
    return;
  }
  const normalized = normalizeUrl(url);
  localStorage.removeItem('file-preview-open');
  panelOverlay.value = { type: 'url-preview', url: normalized };
  webviewInitialUrl.value = normalized;
  revealContentPane();
  pushNavState();
}

/** Open a REAL local filesystem location with the OS — mount a `.dmg`, reveal a
 *  folder in Finder, etc. The desktop build routes through the Tauri
 *  `open_url_external` command (macOS `open`, Windows `rundll32`, Linux
 *  `xdg-open`), which handles `file://` URLs and bare absolute paths uniformly.
 *
 *  This is deliberately NOT `openUrl` / `openFilePreview`: the target lives
 *  OUTSIDE the workspace (e.g. a staged release `.dmg`), so it must not go to
 *  the in-app URL panel webview or the `/data/*` static mount. On a non-Tauri
 *  web build there is no OS bridge — best-effort open in a new tab so we never
 *  crash (browsers ignore `file://`; a real path just dead-ends, same as before
 *  this branch existed). Caller is responsible for classifying the href as a
 *  local file (see `extractLocalFileTarget`). */
export function openLocalFile(target: string): void {
  if (!isTauri()) {
    window.open(target, '_blank', 'noopener');
    return;
  }
  void openExternal(target).catch((err) =>
    showToast(`Couldn't open ${target}: ${errorDetail(err)}`, 'error'),
  );
}

/** Update panelUrl display from in-webview navigation (link clicks, history back/forward).
 *  Does NOT push to the panel nav stack — the webview maintains its own history internally.
 *  Only openUrl/closeUrl push to panel nav (panel-level actions). */
export function updatePanelUrl(url: string): void {
  const o = panelOverlay.value;
  if (o?.type === 'url-preview') {
    panelOverlay.value = { ...o, url };
  }
}

export function closeUrl(): void {
  panelOverlay.value = null;
  webviewInitialUrl.value = null;
  pushNavState();
}

// --- Folder tree builder ---

export interface FolderNode {
  name: string;
  children: Record<string, FolderNode>;
  files: Array<{ name: string; path: string }>;
  path?: string;
}

export function buildFolderTree(paths: string[]): FolderNode {
  const tree: FolderNode = { name: '', children: {}, files: [] };

  for (const path of paths) {
    const parts = path.split('/');
    let node = tree;

    for (let i = 0; i < parts.length - 1; i++) {
      const folderName = parts[i];
      if (!node.children[folderName]) {
        node.children[folderName] = {
          name: folderName,
          children: {},
          files: [],
          path: parts.slice(0, i + 1).join('/'),
        };
      }
      node = node.children[folderName];
    }

    node.files.push({ name: parts[parts.length - 1], path });
  }

  return tree;
}
