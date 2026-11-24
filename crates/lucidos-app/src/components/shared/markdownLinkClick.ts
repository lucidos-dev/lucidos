import type { App } from '../../store/types';
import { openImagePopupFromGroup, showToast, workspacePath } from '../../store/store';
import { openLocalFileOnConfirm } from '../../store/actions/artifacts';
import { openFilePreviewModal } from '../../store/actions/filePreviewModal';
import { openApp, openAppById } from '../../store/actions/apps';
import { navigateToTrigger } from '../../store/actions/triggers';
import { handleNavigationRequest } from '../../store/actions/navigation-request';
import { openRepoFileLink } from '../../store/actions/repoFileLink';
import {
  extractAppTargetFromHref,
  extractNavTargetFromHref,
  extractLocalFileTarget,
  extractBareAppRef,
  extractDataPathTarget,
  extractWorkspaceDiskDataPath,
  extractTriggerIdFromHref,
  extractRepoFileTargetFromHref,
  hasUrlScheme,
  browserHandlesHref,
  extractSettingsTargetFromHref,
  hrefLineRef,
  labelLineRef,
  type LineRef,
} from '../../utils/linkifyPaths';

const DEAD_LINK_TOAST_KEY = 'markdown-dead-link';

function deadLinkMessage(href: string): string {
  if (!href) return 'This link has no destination';
  if (hasUrlScheme(href)) return `Link "${href}" uses a scheme nothing here can open`;
  return `Link "${href}" points nowhere in this workspace`;
}

function citedLines(anchor: HTMLElement, fromHref: LineRef | null): LineRef | null {
  return fromHref ?? labelLineRef(anchor.textContent ?? '');
}

function artifactAnchorLines(anchor: HTMLElement): LineRef | null {
  const { line, lineEnd } = anchor.dataset;
  if (line === undefined) return null;
  return { line: Number(line), lineEnd: lineEnd === undefined ? undefined : Number(lineEnd) };
}

function previewDataFile(path: string, lines: LineRef | null) {
  openFilePreviewModal({ file_path: path, line: lines?.line, line_end: lines?.lineEnd });
}

export function handleMarkdownLinkClick(e: MouseEvent, apps: App[], source?: string) {
  const imgTarget = (e.target as HTMLElement).closest('.image-thumbnail') as HTMLImageElement | null;
  if (imgTarget) {
    e.preventDefault();
    const src = imgTarget.dataset.fullSrc || imgTarget.src;
    if (src) openImagePopupFromGroup(src, imgTarget);
    return;
  }

  const artifactTarget = (e.target as HTMLElement).closest('.artifact-link') as HTMLElement | null;
  if (artifactTarget) {
    e.preventDefault();
    const path = artifactTarget.dataset.path;
    if (path) previewDataFile(path, citedLines(artifactTarget, artifactAnchorLines(artifactTarget)));
    return;
  }

  const appTarget = (e.target as HTMLElement).closest('.app-link') as HTMLElement | null;
  if (appTarget) {
    e.preventDefault();
    const appId = appTarget.dataset.appId;
    if (appId) void openAppById(appId, source, appTarget.dataset.appFragment);
    return;
  }

  const triggerTarget = (e.target as HTMLElement).closest('.trigger-link') as HTMLElement | null;
  if (triggerTarget) {
    e.preventDefault();
    const triggerId = triggerTarget.dataset.triggerId;
    if (triggerId) void navigateToTrigger(triggerId, source);
    return;
  }

  const navTarget = (e.target as HTMLElement).closest('.nav-link') as HTMLElement | null;
  if (navTarget) {
    e.preventDefault();
    const target = navTarget.dataset.navTarget;
    if (target) {
      void handleNavigationRequest({
        target,
        settings_view: navTarget.dataset.settingsView,
        fragment: navTarget.dataset.settingsFragment,
      });
    }
    return;
  }

  const anchorTarget = (e.target as HTMLElement).closest('a') as HTMLAnchorElement | null;
  if (anchorTarget) {
    const rawHref = anchorTarget.getAttribute('href') || '';
    const appTargetRef = extractAppTargetFromHref(rawHref);
    if (appTargetRef) {
      e.preventDefault();
      void openAppById(appTargetRef.appId, source, appTargetRef.fragment ?? undefined);
      return;
    }
    const triggerId = extractTriggerIdFromHref(rawHref);
    if (triggerId) {
      e.preventDefault();
      void navigateToTrigger(triggerId, source);
      return;
    }
    const repoFile = extractRepoFileTargetFromHref(rawHref);
    if (repoFile) {
      e.preventDefault();
      const fromHref = repoFile.line === undefined ? null : { line: repoFile.line, lineEnd: repoFile.lineEnd };
      const lines = citedLines(anchorTarget, fromHref);
      void openRepoFileLink(
        { locator: repoFile.locator, line: lines?.line, lineEnd: lines?.lineEnd },
        source,
        openFilePreviewModal,
      );
      return;
    }
    const settings = extractSettingsTargetFromHref(rawHref);
    if (settings) {
      e.preventDefault();
      void handleNavigationRequest({
        target: 'settings',
        settings_view: settings.view,
        fragment: settings.fragment ?? undefined,
      });
      return;
    }
    const navName = extractNavTargetFromHref(rawHref);
    if (navName) {
      e.preventDefault();
      void handleNavigationRequest({ target: navName });
      return;
    }
    const bareRef = extractBareAppRef(rawHref);
    if (bareRef) {
      const app = apps.find((a) => a.id === bareRef || a.name === bareRef);
      if (app) {
        e.preventDefault();
        openApp(app);
        return;
      }
    }
    const dataPath = extractDataPathTarget(rawHref)
      ?? extractWorkspaceDiskDataPath(rawHref, workspacePath.value);
    if (dataPath) {
      e.preventDefault();
      previewDataFile(dataPath, citedLines(anchorTarget, hrefLineRef(rawHref)));
      return;
    }
    const localFile = extractLocalFileTarget(rawHref);
    if (localFile) {
      e.preventDefault();
      void openLocalFileOnConfirm(localFile, source);
      return;
    }
    if (!browserHandlesHref(rawHref) && !rawHref.startsWith('#')) {
      e.preventDefault();
      showToast(deadLinkMessage(rawHref), 'error', { key: DEAD_LINK_TOAST_KEY });
    }
  }
}
