// IIFE bundle entry — adds iframe-only side effects to keep `index.ts` ES-import-safe for the frontend.
import { installScrollMemory } from './scroll';
import { installKeyboardForwarding } from './keyboardForward';
import { installAppPullToRefresh } from './pullToRefresh';
import { installAppPaneSwipe } from './paneSwipe';
import { installLinkPressGuard } from './linkPressGuard';
import { installAppLinkRouting } from './appLinks';
import { installAppScrollReport } from './appScroll';
import { installTooltips } from './tooltip';
import { primeDevicePreferences } from './ui';
import { installAutocorrectStamp } from './autocorrectStamp';
import { installNoKeyCodeText } from './noKeyCodeText';
import { isBridged } from './_bridge';
import { primeBridgedStorage } from './_storage';
import { installHostOps } from './hostOps';
import { installCapabilityRenewal, installDocumentRelativeUrls } from './frameCapability';
import { installWidgetFrame } from './widgetFrame';
import { installOneSound } from './oneSound';

export * from './index';

if (typeof document !== 'undefined') {
  // The device's Autocorrect switch, on this frame's own text fields. It starts
  // from the first-paint seed or the default of on, and the prime below
  // corrects it to the device's stored value.
  installAutocorrectStamp();
  // An arrow key with nowhere to move the caret types a square in the desktop
  // app. The host's own guard cannot reach this frame, so it refuses here too.
  installNoKeyCodeText();
  // Answer the host's own requests: capture, app switch, fragment delivery. It
  // reached through `contentWindow` for all three until an opaque origin closed
  // that. Installed unconditionally, so the host can ask the same way whichever
  // frame it got.
  installHostOps();
  // Take the renewed pass the host pushes before this one lapses (ADR 0238).
  // The engine seeded the first one into `<base href>`, and each renewal is one
  // attribute write. Nothing reloads, and an already-loaded resource is
  // untouched. A document with no pass ignores every push.
  installCapabilityRenewal();
  // That base names another path, so a bare `#tab` would resolve off this
  // document. History calls and in-page links resolve against its own URL.
  installDocumentRelativeUrls();
  // Report the content size once the host marks this frame a widget, so the
  // widget's frame fits what it draws (ADRs 0402, 0407).
  installWidgetFrame();
  // A sound starting here pauses every other frame's, and the host's. The
  // frames cannot reach each other, so the host relays.
  installOneSound();
  // Scroll memory reads storage, and an isolated frame's own storage throws, so
  // the host holds it. That read is async and the restore is not, so the values
  // are fetched first. `primeBridgedStorage` is a no-op in a frame that can read
  // for itself, which is why the direct path still installs synchronously. It
  // runs once, and `lucidos.storage.ready` waits on this same answer.
  if (isBridged()) {
    primeBridgedStorage()
      .catch((err) => {
        console.warn('[lucidos-sdk] could not read this app\'s stored values:', err);
      })
      // Either way: an empty mirror reads as a first visit, which is a position
      // worth landing at rather than no scroll memory at all from here on.
      .finally(() => { installScrollMemory(); });
  } else {
    installScrollMemory();
  }
  // A themed tooltip on any data-tooltip element, with no init call from the
  // app. The two options differ from the host shell, because an app author
  // never wires a touch affordance by hand: any data-tooltip answers a long
  // press, and the tooltip clears itself shortly after the finger lifts.
  installTooltips({ longPressNeedsOptIn: false, hideAfterLongPressMs: 2000 });
  // Read this device's preferences once, for the two things every app frame
  // follows whether or not it themes itself: the external-link target (the
  // click handler below reads it synchronously, so the "Ask" share sheet keeps
  // the click's user activation) and the Autocorrect switch. Best-effort: on
  // failure links take the host path and the stamp keeps its load-time value.
  primeDevicePreferences().catch((err) => {
    console.warn('[lucidos-sdk] could not read this device\'s preferences:', err);
  });
  // Forward host keyboard shortcuts (pane focus/hide, narrow/widen, Escape, …)
  // up to the parent — iframe keydowns never reach the host document otherwise,
  // so shortcuts die whenever an app has focus. See keyboardForward.ts.
  installKeyboardForwarding();
  // A pull past the top of the app asks the host to refresh it, as the header's
  // Refresh does. The frame captures every touch, so only the SDK can see it.
  installAppPullToRefresh();
  // A sideways drag anywhere in the app swipes the host's panes, as it does
  // over the transcript. Same reason: only the SDK can see the frame's touches.
  installAppPaneSwipe();
  // The app's scroll moves the host's mobile header, as a transcript's does.
  // Same reason again: the frame's own document scrolls, out of the host's sight.
  installAppScrollReport();

  // A swipe that starts on a link must not open it. Installed before the click
  // listener below, so that one never sees the click. A frame has no engine
  // log of its own, so the breadcrumb goes to the console.
  installLinkPressGuard(window, {
    onSwallowed: (info) => console.info('[lucidos-sdk] link-click-after-swipe', info),
  });

  // Links in the frame: downloads, web pages and new-tab targets.
  installAppLinkRouting();
}
