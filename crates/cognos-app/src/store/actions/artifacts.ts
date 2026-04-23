import {
  artifacts,
  artifactRevision,
  expandedFolders,
  uploadProgress,
  panelOverlay,
  webviewInitialUrl,
  filePreviewSource,
} from '../store';
import { loadedOr, toFailed } from '../types';
import { listArtifacts, uploadFile } from '../../api/client';
import { navigateToPane } from './pane';
import { pushNavState } from './navigation';
import { isMobile } from '../../utils/viewport';
import { isTauri } from '../../utils/platform';
import { errorDetail } from '../../utils/errorDetail';

export async function loadArtifacts(): Promise<void> {
  if (artifacts.value.status !== 'loaded') {
    artifacts.value = { status: 'loading' };
  }
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

    // Restore previously open file preview
    const savedPath = localStorage.getItem('file-preview-open');
    if (savedPath && panelOverlay.value?.type !== 'file-preview') {
      if (paths.includes(savedPath)) {
        openFilePreview(savedPath);
      } else {
        localStorage.removeItem('file-preview-open');
      }
    }
  } catch (error) {
    console.error('Failed to load artifacts:', error);
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

export async function uploadFiles(files: FileList): Promise<void> {
  const fileList = Array.from(files);
  const total = fileList.length;
  let succeeded = 0;
  let failed = 0;
  const errors: string[] = [];

  for (let i = 0; i < fileList.length; i++) {
    const file = fileList[i];
    uploadProgress.value = { status: 'uploading', filename: file.name, current: i + 1, total };
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

  uploadProgress.value = { status: 'done', succeeded, failed, errors };

  // Auto-dismiss success after 3.5s; errors stay until manually dismissed
  if (failed === 0) {
    setTimeout(() => { uploadProgress.value = null; }, 3500);
  }

  await loadArtifacts();
}

// --- Path normalization ---

const DATA_PREFIXES = ['artifacts/', 'knowhow/', 'apps/', 'triggers/', 'system-docs/'];

/** Ensure a data path starts with a known directory prefix.
 *  The navigate_ui tool may receive paths without the prefix — normalize
 *  to match the format expected by the /data/* static mount. */
export function normalizeDataPath(path: string): string {
  if (DATA_PREFIXES.some(p => path.startsWith(p))) return path;
  return `artifacts/${path}`;
}

// --- File preview window actions ---

export function openFilePreview(path: string): void {
  filePreviewSource.value = false;
  panelOverlay.value = { type: 'file-preview', path };
  localStorage.setItem('file-preview-open', path);
  if (isMobile()) navigateToPane('content');
  pushNavState();
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
  if (isMobile()) navigateToPane('content');
  pushNavState();
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
