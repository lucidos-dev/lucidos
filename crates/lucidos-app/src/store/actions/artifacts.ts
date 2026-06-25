import {
  artifacts,
  artifactRevision,
  expandedFolders,
  panelOverlay,
  webviewInitialUrl,
  filePreviewSource,
  showToast,
  dismissToast,
} from '../store';
import { loadedOr, toFailed, setLoadingIfFresh } from '../types';
import { listArtifacts, uploadFile } from '../../api/client';
import { revealContentPane } from './pane';
import { pushNavState } from './navigation';
import { isTauri } from '../../utils/platform';
import { openExternal } from '../../utils/tauri';
import { errorDetail } from '../../utils/errorDetail';

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
    artifactRevision.value++;

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

function getArtifactPaths(): string[] {
  return loadedOr(artifacts.value, []);
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

export function expandAllFolders(): void {
  const tree = buildFolderTree(getArtifactPaths());
  const allPaths = new Set<string>();
  collectAllPaths(tree, allPaths);
  expandedFolders.value = allPaths;
}

export function collapseAllFolders(): void {
  expandedFolders.value = new Set();
}

function collectAllPaths(node: FolderNode, paths: Set<string>): void {
  for (const folderName of Object.keys(node.children)) {
    const child = node.children[folderName];
    if (child.path) paths.add(child.path);
    collectAllPaths(child, paths);
  }
}

const UPLOAD_TOAST_KEY = 'upload-progress';

export async function uploadFiles(files: FileList): Promise<void> {
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

const DATA_PREFIXES = ['artifacts/', 'knowhow/', 'apps/', 'triggers/', 'system-knowhow/'];

/** Ensure a data path starts with a known directory prefix.
 *  The navigate_ui tool may receive paths without the prefix — normalize
 *  to match the format expected by the /data/* static mount. */
export function normalizeDataPath(path: string): string {
  if (DATA_PREFIXES.some(p => path.startsWith(p))) return path;
  return `artifacts/${path}`;
}

// --- File preview window actions ---

export function openFilePreview(path: string, opts?: { preserveSource?: boolean }): void {
  if (!opts?.preserveSource) filePreviewSource.value = false;
  panelOverlay.value = { type: 'file-preview', path };
  localStorage.setItem('file-preview-open', path);
  revealContentPane();
  pushNavState();
}

export function refreshFilePreview(): void {
  artifactRevision.value++;
}

// --- URL preview in panel ---

/** Normalize a URL to match Rust's url::Url normalization (trailing slash, lowercase, etc.)
 *  so that URL comparisons between frontend and backend are consistent. */
export function normalizeUrl(url: string): string {
  try { return new URL(url).href; } catch { return url; }
}

export function openUrl(url: string): void {
  const normalized = normalizeUrl(url);
  if (!isTauri()) {
    window.open(normalized, '_blank', 'noopener');
    return;
  }
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
