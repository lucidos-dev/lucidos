import { switchMenuItem, openSettingsSubview, setActiveMenu } from './menu';
import { openAppById } from './apps';
import { openFilePreview, openUrl } from './artifacts';
import { resolveFileTarget } from './fileTarget';
import { openOAuthAuthorizationUrl } from './oauth';
import { navigateToTrigger } from './triggers';
import { focusThreadOrBootstrap, unfocusThread } from './threads';
import { pushNavState } from './navigation';
import { revealContentPane } from './pane';
import { ensureFocusedComposeThread, updateCompose } from './compose';
import { openEncodedRepoFilePreview } from './repositories';
import { focusPromptNow } from '../../components/chat/promptFocus';
import { showToast, panelOverlay, pluginScrollTarget, setPluginsInstalledOnly, parseRepoPath, normalizeLineRange, selectedLines, lineScrollTarget, filePreviewSource, SETTINGS_NAV_ITEMS, SETTINGS_SYSTEM_SUBPANEL_ITEMS, settingsSubviewLabel, aliasRetiredSettingsSubview } from '../store';
import type { SettingsNavKey } from '../store';
import type { MenuItem } from '../types';

/** Is `view` a renderable Settings sub-section (`SettingsView.renderSubview` has
 *  a case for each)? The valid set is derived from the store's own nav lists —
 *  the single source of truth for what Settings shows — so a new subview added
 *  there is accepted here automatically (no second list to keep in sync). It is a
 *  superset of the engine's advertised `settings_view` set; a Vitest pins
 *  advertised ⊆ this set. Computed lazily (memoized) so importing this module
 *  never touches those store exports at load time — that would force every
 *  `../store`-mocking test transitively importing this file to stub them. */
let settingsViewKeys: ReadonlySet<string> | null = null;
function isRenderableSettingsView(view: string): boolean {
  if (settingsViewKeys === null) {
    settingsViewKeys = new Set<string>([
      ...SETTINGS_NAV_ITEMS.map((i) => i.key),
      ...SETTINGS_SYSTEM_SUBPANEL_ITEMS.map((i) => i.key),
    ]);
  }
  return settingsViewKeys.has(view);
}

/** Handle a NavigationRequested event — dispatches to the correct UI action based on target.
 *
 *  Shared by two callers: the `NavigationRequested` ThreadEvent path (engine
 *  asking the page to navigate, e.g. after an LLM `navigate_ui` tool call)
 *  AND the structured-tap dispatcher in `in-app-notification-toast.ts` (the
 *  `tap.kind === 'navigate'` branch). Both flow through one router so the
 *  destination semantics stay identical regardless of trigger.
 *
 *  Each branch checks for the target it needs to navigate to and surfaces
 *  a "<destination> no longer exists" toast when it's gone. Targets without
 *  an id-based lookup (panels, creation forms, raw urls) trust their inputs. */
/** Short human phrase for a navigation destination — used by the off-focus
 *  "Thread X wants to open …" jump offer so the toast names where it'd go. */
export function describeNavTarget(nav: {
  target: string;
  app_id?: string;
  file_path?: string;
  purpose?: string;
  line?: number;
  line_end?: number;
  settings_view?: string;
}): string {
  switch (nav.target) {
    case 'app':
    case 'app-ui':
      return nav.app_id ? `app "${nav.app_id}"` : 'an app';
    case 'thread':
      return 'a thread';
    case 'file': {
      if (!nav.file_path) return 'a file';
      // A repo-encoded path (`repo:<repoId>:file:<path>`) names the repo file
      // it points at — the raw encoding would read as a uuid soup in the toast.
      const repo = parseRepoPath(nav.file_path);
      // A cited line is the point of the jump, so the offer names it the way
      // the citation did (`file "src/main.rs:510"`). A range collapses to its
      // first line: the offer is a destination, not a selection.
      const range = normalizeLineRange(nav.line, nav.line_end);
      const at = range ? `:${range.start}` : '';
      return `file "${repo?.path ?? nav.file_path}${at}"`;
    }
    case 'trigger':
      return 'a trigger';
    case 'url':
      // An OAuth authorization page is worth naming: unlike an ordinary link it
      // EXPIRES (the engine's callback listener gives up after 120s), so a user
      // deciding whether to tap Open needs to know this one is time-bounded.
      return nav.purpose === 'oauth' ? 'a sign-in page (expires shortly)' : 'a link';
    case 'new-app':
      return 'the new-app form';
    case 'new-trigger':
      return 'the new-trigger form';
    case 'new-chat':
      return 'a new chat';
    case 'settings': {
      if (!nav.settings_view) return 'settings';
      const view = aliasRetiredSettingsSubview(nav.settings_view);
      const label = settingsSubviewLabel(view as SettingsNavKey);
      return `${label ?? nav.settings_view} settings`;
    }
    case 'app-store':
      return 'the Plugins panel';
    case 'plugins':
      return 'your installed plugins';
    case 'thread-queue':
      return 'the Thread Queue';
    case 'files':
    case 'apps':
    case 'triggers':
    case 'changes':
    case 'notifications':
      return `the ${nav.target} panel`;
    default:
      return 'a page';
  }
}

export function handleNavigationRequest(nav: {
  target: string;
  settings_view?: string;
  app_id?: string;
  file_path?: string;
  line?: number;
  line_end?: number;
  url?: string;
  /** Why this URL is being opened. Only `'oauth'` is meaningful today: it marks
   *  an authorization page whose in-app panel should close once the flow lands.
   *  Absent for every ordinary link. */
  purpose?: string;
  id?: string;
  event_id?: string;
  prompt?: string;
}, opts?: { source?: string }): void {
  const navAppId = nav.app_id;
  switch (nav.target) {
    case 'files':
    case 'apps':
    case 'triggers':
    case 'changes':
    case 'notifications':
      switchMenuItem(nav.target as MenuItem);
      break;
    case 'thread-queue':
      // The Thread Queue lives under Settings → System (no longer a top-level
      // panel). The `thread-queue` NavigateTarget stays stable in the SDK +
      // engine (notification overflow taps / navigate_ui still emit it); the
      // frontend just reinterprets it to land on the System subpanel.
      switchMenuItem('settings');
      openSettingsSubview('thread-queue');
      break;
    case 'app-store':
      // Plugins (and the apps they ship) are downloaded from the Plugins panel.
      // Older notifications (and the still-valid `app-store` NavigateTarget)
      // deep-link here — land on Plugins with the whole catalog showing (the
      // All | Installed toggle on All).
      setPluginsInstalledOnly(false);
      switchMenuItem('plugins');
      break;
    case 'plugins':
      // Land on the Plugins panel narrowed to Installed (the All | Installed
      // toggle on Installed) — where every installed plugin (app or not) shows
      // its update status + an Update button. The update-available notification
      // deep-links here and carries the plugin id in `nav.id`; the list scrolls
      // to and pulses that row once it renders (see StoreTab's pluginScrollTarget
      // effect).
      setPluginsInstalledOnly(true);
      switchMenuItem('plugins');
      if (nav.id) pluginScrollTarget.value = nav.id;
      break;
    case 'settings':
      switchMenuItem('settings');
      if (nav.settings_view) {
        // A request from OUTSIDE this build can name a subview a restructure
        // has since moved: a stored notification's deep link, an app compiled
        // against an older SDK `SettingsViewTarget`, an LLM working from a
        // stale schema. Alias those onto the category that absorbed them
        // (`repositories` → Coding Agents, `mobile-access` → Access, …) so an
        // old payload still lands where the user expects.
        //
        // Then validate against the renderable set, don't trust the input: an
        // unknown subview would otherwise land on a blank settings panel
        // (a silent error). Fail loud instead; we still showed Settings home.
        // The toast names what the CALLER sent, not the alias, so the sender
        // can see which value was wrong.
        const view = aliasRetiredSettingsSubview(nav.settings_view);
        if (isRenderableSettingsView(view)) {
          openSettingsSubview(view as SettingsNavKey);
        } else {
          showToast(`Unknown settings section: "${nav.settings_view}"`, 'error');
        }
      }
      break;
    case 'app':
    case 'app-ui':
      // `app-ui` is a historical alias of `app` — current producers (LLM
      // tool, structured Tap) only emit `app`, but old NavigationRequested
      // events and stale notification rows may still carry `app-ui`.
      if (!navAppId) {
        showToast('Navigation target missing app_id', 'error');
        break;
      }
      // Delegate straight to openAppById — do NOT pre-check the cached
      // `appsList`. That list is a disk-backed projection refreshed by App*
      // SSE events; a sibling thread that just created the app via raw file
      // writes leaves it momentarily stale, and the old pre-check reported a
      // live app as "App no longer exists" — swallowing the real cause.
      // openAppById re-scans disk on a cache miss before erroring, and its
      // miss toast names the app id + where the navigate came from.
      void openAppById(navAppId, opts?.source);
      break;
    case 'file': {
      // file_path existence is server-checked at preview time; the API surface
      // emits its own toast on miss. Up-front check is just for presence.
      if (!nav.file_path) {
        showToast('Navigation target missing file_path', 'error');
        break;
      }
      // One resolver for the locator and its line rules, shared with the
      // app-facing preview modal so the two surfaces address exactly the same
      // set of files (see `resolveFileTarget`).
      const { path: filePath, range } = resolveFileTarget(nav.file_path, nav.line, nav.line_end);
      // A repo-encoded path (`repo:<repoId>:file:<path>`) opens a file from a
      // registered repository clone rather than the workspace data tree, and
      // needs that repo bound before the panel mounts — see
      // openEncodedRepoFilePreview, which returns false for anything else so a
      // plain data path falls through to the /data/* preview.
      if (!openEncodedRepoFilePreview(filePath)) openFilePreview(filePath);
      // A cited line, applied AFTER the open: both openers deliberately clear
      // `selectedLines` so a previous file's range can't leak onto this one, and
      // openFilePreview resets `filePreviewSource`. Setting the range here is
      // what keeps that clearing intact while still landing on the line.
      //
      // Source view is forced because a format that renders (markdown, CSV, SVG)
      // has no lines to highlight. Without it, every citation into a markdown
      // file would silently ignore its line. A target that can never show lines
      // (a PDF, an image, a diff) resolves to a null range and just opens, which
      // is the documented degradation: an unusable line never withholds the file.
      if (range) {
        filePreviewSource.value = true;
        selectedLines.value = range;
        lineScrollTarget.value = range.start;
      }
      break;
    }
    case 'trigger':
      if (!nav.id) {
        showToast('Navigation target missing trigger id', 'error');
        break;
      }
      // Delegate straight to navigateToTrigger — do NOT pre-check the cached
      // `triggers` list. That list is a projection refreshed by Trigger* SSE
      // events; a sibling thread that just created the trigger leaves it
      // momentarily stale, and a pre-check would report a live trigger as
      // "no longer exists" — swallowing the real cause. navigateToTrigger
      // re-fetches the source of truth on a cache miss before erroring, and
      // its miss toast names the trigger id + where the navigate came from.
      // (Mirrors the `app` branch above.)
      void navigateToTrigger(nav.id, opts?.source);
      break;
    case 'thread':
      // focusThreadOrBootstrap (not focusThread): the thread may live outside
      // the loaded window (old archived row, cross-workspace deep link). It
      // fetches metadata, navigates on success, and surfaces "Thread not
      // found" on miss. event_id forwards to scroll-and-pulse the source
      // event on land (typically a UserQuestionAsked / CodingAgentPermission).
      if (!nav.id) {
        showToast('Navigation target missing thread id', 'error');
        break;
      }
      focusThreadOrBootstrap(nav.id, { targetEventId: nav.event_id ?? null });
      break;
    case 'new-app':
      // Single nav push — switchMenuItem would push (apps, no overlay) first,
      // stranding Back on an empty Apps list. setActiveMenu is pure plumbing
      // (no pane logic), so we own the revealContentPane() call here — without
      // it, mobile users tapping a new-app deep-link silently stayed on
      // whatever pane they were on.
      setActiveMenu('apps', { type: 'form', form: { type: 'new-app' } });
      pushNavState();
      revealContentPane();
      break;
    case 'new-trigger':
      setActiveMenu('triggers', { type: 'form', form: { type: 'trigger' } });
      pushNavState();
      revealContentPane();
      break;
    case 'new-chat': {
      // Close any open overlay (app, file preview, settings panel) so the
      // chat panel underneath becomes the visible target for the prefill.
      panelOverlay.value = null;
      // Drop any focused thread so ensureFocusedComposeThread allocates a
      // fresh one — without this it returns the existing focused id and the
      // prefill would land on whatever thread the user was viewing.
      unfocusThread();
      const id = ensureFocusedComposeThread();
      if (typeof nav.prompt === 'string' && nav.prompt.length > 0) {
        updateCompose(id, { text: nav.prompt });
      }
      // rAF — Preact hasn't committed the panelOverlay/unfocusThread mutations
      // yet, so a sync focus would query the pre-render DOM and miss (or hit
      // the wrong layout's) prompt-input element. Mirrors the keyboard
      // shortcut path in useKeyboardShortcuts.ts.
      requestAnimationFrame(() => focusPromptNow());
      break;
    }
    case 'url':
      if (!nav.url) {
        showToast('Navigation target missing url', 'error');
        break;
      }
      // `purpose: 'oauth'` marks the one URL that has an end: an authorization
      // page the provider will redirect to the loopback callback. Recording it
      // is what lets `handleOAuthAccountConnected` close the in-app browser
      // panel afterwards, instead of leaving the user on a dead callback page
      // inside the app. Every other URL is just opened.
      if (nav.purpose === 'oauth') {
        openOAuthAuthorizationUrl(nav.url);
      } else {
        // `source` is carried so a browser that refuses the tab can say which
        // thread (or app) asked for it. A navigate arriving over SSE is not
        // something the user clicked, so an unattributed toast would read as
        // coming from nowhere.
        openUrl(nav.url, opts?.source);
      }
      break;
    default:
      // Unknown target — log via toast so a future schema drift surfaces
      // loudly instead of silently no-op'ing.
      showToast(`Unknown navigation target: ${nav.target}`, 'error');
      break;
  }
}
