import { readWindowItem, removeWindowItem, writeWindowItem } from '../../utils/windowStorage';
import {
  artifacts,
  filePreviewRevision,
  expandedFolders,
  panelOverlay,
  webviewInitialUrl,
  filePreviewSource,
  selectedLines,
  lineScrollTarget,
  findRequest,
  showToast,
  showConfirm,
  dismissToast,
  parseRepoPath,
} from '../store';
import { toFailed, setLoadingIfFresh } from '../types';
import { listArtifacts, uploadErrorMessage, uploadFile } from '../../api/client';
import { revealContentPane } from './pane';
import { pushNavState } from './navigation';
import { isTauri } from '../../utils/platform';
import { externalWebUrl } from '@lucidos/web-link';
import { openExternalUrl, refuseDangerousUrl } from '../../utils/openExternalUrl';
import { DATA_PATH_PREFIXES } from '../../utils/dataPathPrefixes';
import { openExternal } from '../../utils/tauri';
import { errorDetail } from '../../utils/errorDetail';
import { inAppBrowserAvailable } from './preferences';

let filePreviewRestoreAttempted = false;

let listingInFlight: Promise<void> | null = null;
let listAgain = false;

export function loadArtifacts(): Promise<void> {
  if (listingInFlight) {
    listAgain = true;
    return listingInFlight;
  }
  listingInFlight = (async () => {
    try {
      do {
        listAgain = false;
        await listArtifactsOnce();
      } while (listAgain);
    } finally {
      listingInFlight = null;
    }
  })();
  return listingInFlight;
}

export function refreshArtifacts(): void {
  if (artifacts.value.status !== 'not-loaded') void loadArtifacts();
}

function sameList(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((path, i) => path === b[i]);
}

async function listArtifactsOnce(): Promise<void> {
  setLoadingIfFresh(artifacts);
  try {
    const data = await listArtifacts();
    const paths = data.artifacts || [];
    const prev = artifacts.value;
    if (prev.status !== 'loaded' || !sameList(prev.data, paths)) {
      artifacts.value = { status: 'loaded', data: paths };
    }

    if (expandedFolders.value.size === 0 && paths.length > 0) {
      const tree = buildFolderTree(paths);
      const newExpanded = new Set<string>();
      for (const folderName of Object.keys(tree.children)) {
        newExpanded.add(folderName);
      }
      expandedFolders.value = newExpanded;
    }

    if (!filePreviewRestoreAttempted) {
      filePreviewRestoreAttempted = true;
      const savedPath = readWindowItem('file-preview-open');
      if (savedPath && panelOverlay.value?.type !== 'file-preview') {
        if (paths.includes(savedPath)) {
          openFilePreview(savedPath, { preserveSource: true });
        } else {
          removeWindowItem('file-preview-open');
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
      errors.push(`${file.name}: ${uploadErrorMessage(error)}`);
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
    showToast(`${summary}: ${errors.join('; ')}`, 'error', { key: UPLOAD_TOAST_KEY });
  }

  await loadArtifacts();
}


export function normalizeDataPath(path: string): string {
  if (parseRepoPath(path)) return path;
  if (DATA_PATH_PREFIXES.some(p => path.startsWith(p))) return path;
  return `artifacts/${path}`;
}


export function openFilePreview(path: string, opts?: { preserveSource?: boolean }): void {
  if (!opts?.preserveSource) filePreviewSource.value = false;
  selectedLines.value = null;
  lineScrollTarget.value = null;
  findRequest.value = null;
  panelOverlay.value = { type: 'file-preview', path };
  writeWindowItem('file-preview-open', path);
  revealContentPane();
  pushNavState();
}

let previewBumpDebounce: ReturnType<typeof setTimeout> | null = null;
const PREVIEW_BUMP_DEBOUNCE_MS = 150;

let previewTextBodies = 0;

let previewSettleWaiters: Array<{ rev: number; settle: () => void }> = [];

function settlePreviewRefreshes(upTo: number): void {
  const waiting = previewSettleWaiters;
  previewSettleWaiters = waiting.filter((w) => w.rev > upTo);
  for (const w of waiting) if (w.rev <= upTo) w.settle();
}

export function registerPreviewTextBody(): () => void {
  previewTextBodies++;
  return () => {
    previewTextBodies--;
    if (previewTextBodies === 0) settlePreviewRefreshes(Infinity);
  };
}

export function reportPreviewTextSettled(revision: number): void {
  settlePreviewRefreshes(revision);
}

let pendingBumpSettles: Array<() => void> = [];

function bumpOpenPreview(path: string, onSettled?: () => void): void {
  if (previewBumpDebounce) clearTimeout(previewBumpDebounce);
  if (onSettled) pendingBumpSettles.push(onSettled);
  previewBumpDebounce = setTimeout(() => {
    previewBumpDebounce = null;
    const settles = pendingBumpSettles;
    pendingBumpSettles = [];
    const current = filePreviewRevision.peek();
    const rev = current?.path === path ? current.rev + 1 : 1;
    if (previewTextBodies > 0) previewSettleWaiters.push(...settles.map((settle) => ({ rev, settle })));
    else for (const settle of settles) settle();
    filePreviewRevision.value = { path, rev };
  }, PREVIEW_BUMP_DEBOUNCE_MS);
}

export function invalidateFilePreview(path: string): void {
  const overlay = panelOverlay.peek();
  if (overlay?.type !== 'file-preview' || overlay.path !== path) return;
  bumpOpenPreview(path);
}

export function refreshFilePreview(): Promise<void> {
  const overlay = panelOverlay.peek();
  if (overlay?.type !== 'file-preview') return Promise.resolve();
  return new Promise((resolve) => bumpOpenPreview(overlay.path, resolve));
}


export function normalizeUrl(url: string): string {
  try { return new URL(url).href; } catch { return url; }
}

export function openUrlOutsideApp(url: string, source?: string): void {
  if (refuseDangerousUrl(url, source)) return;
  const normalized = normalizeUrl(url);
  if (!isTauri()) {
    openExternalUrl(normalized, source);
    return;
  }
  if (osOpenerMayLaunchAProgram(normalized)) {
    void openLocalFileOnConfirm(normalized, source);
    return;
  }
  const from = source ? ` (requested by ${source})` : '';
  void openExternal(normalized).catch((err) =>
    showToast(`Couldn't open ${normalized}${from}: ${errorDetail(err)}`, 'error'),
  );
}

const IN_APP_BROWSER_SCHEMES = new Set(['http:', 'https:']);

export function inAppBrowserRenders(url: string): boolean {
  try {
    return IN_APP_BROWSER_SCHEMES.has(new URL(url).protocol);
  } catch {
    return false;
  }
}

export function openUrl(url: string, source?: string): void {
  if (refuseDangerousUrl(url, source)) return;
  if (!inAppBrowserAvailable() || !inAppBrowserRenders(url)) {
    openUrlOutsideApp(url, source);
    return;
  }
  const normalized = normalizeUrl(url);
  removeWindowItem('file-preview-open');
  panelOverlay.value = { type: 'url-preview', url: normalized };
  webviewInitialUrl.value = normalized;
  revealContentPane();
  pushNavState();
}

export function openLocalFile(target: string, source?: string): void {
  if (!isTauri()) {
    const webUrl = externalWebUrl(target);
    if (webUrl) {
      openExternalUrl(webUrl, source);
      return;
    }
    const from = source ? ` (requested by ${source})` : '';
    showToast(
      `${target} is on the computer running Lucidos, outside this workspace, so this browser can't open it${from}`,
      'error',
    );
    return;
  }
  void openExternal(target).catch((err) =>
    showToast(`Couldn't open ${target}: ${errorDetail(err)}`, 'error'),
  );
}

const OS_OPENER_WEB_SCHEMES = new Set(['http:', 'https:', 'mailto:', 'tel:']);

function osOpenerMayLaunchAProgram(target: string): boolean {
  try {
    return !OS_OPENER_WEB_SCHEMES.has(new URL(target).protocol);
  } catch {
    return true;
  }
}

export async function openLocalFileOnConfirm(target: string, source?: string): Promise<void> {
  if (!isTauri()) {
    openLocalFile(target, source);
    return;
  }
  const from = source ? `\n\nRequested by ${source}.` : '';
  const open = await showConfirm(
    `Open ${target} on this computer, with the app your system picks for it?${from}`,
    'Open',
    { variant: 'default', title: 'Open outside Lucidos?' },
  );
  if (open) openLocalFile(target);
}

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
