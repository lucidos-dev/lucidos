import type { App } from '../../store/types';
import { openImagePopupFromGroup, showToast } from '../../store/store';
import { openFilePreview, openLocalFile } from '../../store/actions/artifacts';
import { openApp, openAppById } from '../../store/actions/apps';
import { navigateToTrigger } from '../../store/actions/triggers';
import { handleNavigationRequest } from '../../store/actions/navigation-request';
import {
  extractAppTargetFromHref,
  extractNavTargetFromHref,
  extractLocalFileTarget,
  extractBareAppRef,
  extractDataPathTarget,
  extractTriggerIdFromHref,
  hasUrlScheme,
  browserHandlesHref,
} from '../../utils/linkifyPaths';

/** Toast key for the terminal dead-link guard, so tapping the same dead link
 *  repeatedly replaces the toast rather than stacking. */
const DEAD_LINK_TOAST_KEY = 'markdown-dead-link';

/** What the terminal guard says about an href it swallowed. Three cases, named
 *  apart because they have different fixes. An unopenable SCHEME is usually one
 *  the agent invented. An unresolved relative href usually names something that
 *  moved. An empty one cannot be quoted at all. */
function deadLinkMessage(href: string): string {
  if (!href) return 'This link has no destination';
  if (hasUrlScheme(href)) return `Link "${href}" uses a scheme nothing here can open`;
  return `Link "${href}" points nowhere in this workspace`;
}

/** Route a click inside rendered agent markdown (`linkifyPaths(renderMarkdown(…))`)
 *  to the host surface it names. Every surface that renders that HTML attaches
 *  this one handler, so a link that opens in the transcript opens everywhere.
 *  `source` names the surface in a not-found toast, e.g. "a notification". */
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
    if (path) openFilePreview(path);
    return;
  }

  const appTarget = (e.target as HTMLElement).closest('.app-link') as HTMLElement | null;
  if (appTarget) {
    e.preventDefault();
    const appId = appTarget.dataset.appId;
    // openAppById, not a `apps.find(...)` on the cached list: it re-fetches
    // the registry on a miss before concluding the app is gone, matching the
    // trigger branch below. A suspended iOS PWA can miss the AppCreated SSE
    // frame, so the cache lags a freshly created app until this refetch.
    // `data-app-fragment` is the app fragment the link named, absent when it
    // named none, so a plain app link leaves an open app where it was.
    if (appId) void openAppById(appId, source, appTarget.dataset.appFragment);
    return;
  }

  const triggerTarget = (e.target as HTMLElement).closest('.trigger-link') as HTMLElement | null;
  if (triggerTarget) {
    e.preventDefault();
    const triggerId = triggerTarget.dataset.triggerId;
    // navigateToTrigger, not a `triggers.find(...)` on the cached list: it
    // re-fetches the registry on a miss before concluding the trigger is
    // gone, and names it in the toast if it really is.
    if (triggerId) void navigateToTrigger(triggerId, source);
    return;
  }

  const navTarget = (e.target as HTMLElement).closest('.nav-link') as HTMLElement | null;
  if (navTarget) {
    e.preventDefault();
    const target = navTarget.dataset.navTarget;
    if (target) handleNavigationRequest({ target });
    return;
  }

  // Defense-in-depth: intercept plain anchors whose href points at an app
  // folder (apps/<id>/...) or a Lucidos navigation panel
  // (notifications, apps, triggers, …) even when linkifyPaths didn't
  // rewrite them. Catches: stale memo result rendered before the apps
  // list loaded; iOS PWA JS bundle predating the rewriter; any markdown
  // link the LLM writes. Without this the browser navigates to the relative
  // URL: an app entry lands on a file preview via the engine's /data/* static
  // mount, and a panel name on a 404 for a /data/<panel> folder.
  const anchorTarget = (e.target as HTMLElement).closest('a') as HTMLAnchorElement | null;
  if (anchorTarget) {
    const rawHref = anchorTarget.getAttribute('href') || '';
    const appTargetRef = extractAppTargetFromHref(rawHref);
    // Unconditional, like the .app-link branch above: openAppById re-fetches
    // on a cache miss. A recognized `app:` href must never fall through to
    // the terminal guard, which would blame the SCHEME for a stale cache.
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
    const navName = extractNavTargetFromHref(rawHref);
    if (navName) {
      e.preventDefault();
      handleNavigationRequest({ target: navName });
      return;
    }
    // A bare app-id/name href like `habit-tracker`: the LLM writes
    // `[Habit Tracker](habit-tracker)` by analogy to `[Notifications](notifications)`.
    // Not caught by extractAppTargetFromHref (no apps/ prefix, no app: scheme);
    // resolve it against the loaded apps list by id OR name. Runs AFTER nav so
    // reserved panel names still route to their panel. Without this the
    // browser navigates to the relative href, the SPA fallback serves the
    // shell, and the whole workspace reloads.
    const bareRef = extractBareAppRef(rawHref);
    if (bareRef) {
      const app = apps.find((a) => a.id === bareRef || a.name === bareRef);
      if (app) {
        e.preventDefault();
        openApp(app);
        return;
      }
    }
    // A path under the workspace's data/ tree, recognized by SHAPE. So it
    // works for a file the agent wrote seconds ago that the cached artifact
    // list has not caught up with. That is exactly when `linkifyPaths`
    // declines to rewrite, and this fallback is all that stands between the
    // click and a full workspace reload. Runs BEFORE the OS-open branch, so
    // an absolute /artifacts/… is never mistaken for a disk path.
    const dataPath = extractDataPathTarget(rawHref);
    if (dataPath) {
      e.preventDefault();
      openFilePreview(dataPath);
      return;
    }
    // A `file://` URL or an absolute filesystem path, such as a staged
    // release .dmg or an /Applications/… folder. Open it with the OS, never
    // via the in-app file preview or the /data/* static mount, which are for
    // workspace-relative paths only. Runs AFTER the app and nav extractors so
    // their absolute routes keep working. An http(s) URL returns null here
    // and keeps its browser or panel behavior.
    const localFile = extractLocalFileTarget(rawHref);
    if (localFile) {
      e.preventDefault();
      openLocalFile(localFile);
      return;
    }
    // TERMINAL GUARD: nothing above claimed this href, so it goes nowhere
    // useful. The branches above are a whitelist, and a whitelist is open at
    // the bottom. Closing it here makes the next unrecognized shape a toast
    // the user can read. See ADR 0038.
    //
    // Two failure modes, one guard. A href with NO scheme is a relative link
    // into the SPA, and there are no relative routes: the browser resolves it
    // against the workspace base, the SPA fallback answers with the app
    // shell, and the whole workspace reloads. A href whose scheme nothing can
    // open does nothing at all, and that is the worse of the two: the user
    // cannot tell it from a dead app. `trigger:` was this until it was
    // claimed, and the agent will invent another.
    //
    // Only a fragment (`#section`, which navigates nothing) and a scheme the
    // browser genuinely acts on pass through.
    //
    // An EMPTY href is still swallowed, since `[text]()` resolves to the
    // current URL and reloads exactly like any other unclaimed relative href.
    // It just can't be named in the message. Keyed so tapping the same dead
    // link twice replaces the toast instead of stacking a duplicate.
    if (!browserHandlesHref(rawHref) && !rawHref.startsWith('#')) {
      e.preventDefault();
      showToast(deadLinkMessage(rawHref), 'error', { key: DEAD_LINK_TOAST_KEY });
    }
  }
}
