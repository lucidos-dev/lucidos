import { switchMenuItem, openSettingsSubview, setActiveMenu } from './menu';
import { openAppById } from './apps';
import { openFilePreview, openUrl, normalizeDataPath } from './artifacts';
import { navigateToTrigger } from './triggers';
import { focusThreadOrBootstrap, unfocusThread } from './threads';
import { pushNavState } from './navigation';
import { revealContentPane } from './pane';
import { ensureFocusedComposeThread, updateCompose } from './compose';
import { focusPromptNow } from '../../components/chat/promptFocus';
import { showToast, appsList, triggers, panelOverlay } from '../store';
import type { MenuItem } from '../types';

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
export function handleNavigationRequest(nav: {
  target: string;
  settings_view?: string;
  app_id?: string;
  file_path?: string;
  url?: string;
  id?: string;
  event_id?: string;
  prompt?: string;
}): void {
  const navAppId = nav.app_id;
  switch (nav.target) {
    case 'files':
    case 'apps':
    case 'triggers':
    case 'changes':
    case 'notifications':
      switchMenuItem(nav.target as MenuItem);
      break;
    case 'settings':
      switchMenuItem('settings');
      if (nav.settings_view) {
        openSettingsSubview(nav.settings_view as 'devices' | 'accounts' | 'backup' | 'memory' | 'repositories');
      }
      break;
    case 'app':
    case 'app-ui':
      // `app-ui` is a historical alias of `app` — current producers (LLM
      // tool, structured Tap) only emit `app`, but old NavigationRequested
      // events and stale notification rows may still carry `app-ui`.
      // openAppById lazy-loads the list and surfaces its own "Couldn't open
      // app — no app with id …" toast on miss. We pre-check here so a
      // known-bad id reports as the canonical "App no longer exists".
      if (!navAppId) {
        showToast('Navigation target missing app_id', 'error');
        break;
      }
      {
        const apps = appsList.value;
        if (apps.status === 'loaded' && !apps.data.some((a) => a.id === navAppId)) {
          showToast('App no longer exists', 'error');
        } else {
          // openAppById toasts on failure itself (load-apps miss, unknown id).
          void openAppById(navAppId);
        }
      }
      break;
    case 'file':
      // file_path existence is server-checked at preview time; the API surface
      // emits its own toast on miss. Up-front check is just for presence.
      if (!nav.file_path) {
        showToast('Navigation target missing file_path', 'error');
        break;
      }
      openFilePreview(normalizeDataPath(nav.file_path));
      break;
    case 'trigger':
      if (!nav.id) {
        showToast('Navigation target missing trigger id', 'error');
        break;
      }
      {
        const list = triggers.value;
        if (list.status === 'loaded' && !list.data.some((t) => t.id === nav.id)) {
          showToast('Trigger no longer exists', 'error');
        } else {
          // navigateToTrigger awaits loadTriggers internally on cold-load;
          // loadTriggers surfaces failures via Loadable failed.
          void navigateToTrigger(nav.id);
        }
      }
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
      openUrl(nav.url);
      break;
    default:
      // Unknown target — log via toast so a future schema drift surfaces
      // loudly instead of silently no-op'ing.
      showToast(`Unknown navigation target: ${nav.target}`, 'error');
      break;
  }
}
