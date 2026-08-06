import type { InlineForm, PanelOverlay } from '../../store/store';

/** Short opaque digest of free text (FNV-1a, 32-bit, hex).
 *
 *  Used where a form's identity is a tuple of prose rather than an id. The view
 *  key is not confined to memory: `contentScrollKey` prefixes it into a
 *  localStorage key, and nothing ever prunes those, so a raw tuple would park
 *  the text itself in client storage indefinitely and at unbounded length. Only
 *  distinctness is needed here, and a collision costs exactly what the coarse
 *  key cost before it (one shared scroll position, one missing cover), never
 *  correctness, so a non-cryptographic digest is the right size of tool. */
function digest(text: string): string {
  let hash = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    hash ^= text.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0).toString(16);
}

/** What distinguishes one inline form from another.
 *
 *  The overlay type is `'form'` for all seven of them, so without this a walk
 *  from one trigger's form to another (Back/Forward through the nav history, or
 *  an agent opening a second one over the first) reads as no navigation at all:
 *  the pane restores the outgoing form's scrollTop onto the incoming one and
 *  skips the navigation cover on a swap the user can plainly see.
 *
 *  Exhaustive by construction rather than by a `default` branch: the declared
 *  `string` return makes `tsc` reject a new variant that says nothing about what
 *  identifies it, instead of letting it collapse silently into its siblings. */
export function inlineFormKey(form: InlineForm): string {
  switch (form.type) {
    // Which of the two the credential form is doing is part of its identity, so
    // the branch is in the key: editing the stored "github" credential and an
    // agent's fresh request for the "github" service are different panels, and
    // a bare `editing ?? service` would hand them the same one.
    case 'credential':
      if (form.editing) return `credential:edit:${form.editing}`;
      return `credential:new:${form.request?.service ?? ''}`;
    case 'app-edit': return `app-edit:${form.appId}`;
    case 'new-app': return 'new-app';
    case 'trigger': return `trigger:${form.triggerId ?? 'new'}`;
    // The one request with no id of its own, so who it is addressed to and what
    // it says IS its identity, digested because it is a draft email and this key
    // outlives the panel in localStorage. Deliberately not `sentAt`: the panel
    // turning into a receipt is the same panel, and keying on that would
    // re-cover it (and reset its scroll) at the moment the mail went out.
    case 'email-confirm':
      return `email-confirm:${digest([form.request.account, form.request.to.join(','), form.request.subject].join('|'))}`;
    // Deliberately not the receipt marker, for the same reason `email-confirm`
    // above ignores `sentAt`: a panel flipping to its receipt is the same panel
    // mutating in place, and keying on the marker would re-cover it (and reset
    // its scroll) at the exact moment the files landed or went.
    case 'plugin-install': return `plugin-install:${form.request.install_id}`;
    case 'plugin-uninstall': return `plugin-uninstall:${form.request.uninstall_id}`;
  }
}

/** Identity of whatever the content pane is currently showing: the one answer to
 *  "has this pane navigated". Both consumers key off it, and they have to agree,
 *  or the pane restores a scroll position from a view it no longer shows, or
 *  covers one it never left. Returns null when there is nothing to key on, so
 *  the scroll memory skips and the navigation cover has no arriving view to
 *  hide.
 *
 *  `app-ui` is deliberately ONE key for every app, the only overlay not resolved
 *  down to the thing it displays. An app switch keeps the same iframe element
 *  and navigates it, so the frame's own load cover re-covers it and holds until
 *  the incoming document's `load` (`.app-ui-cover`, AppUiInline.tsx), which is
 *  strictly better than a timed fade over a frame that may still be blank.
 *  There is no scroll position to keep apart either: the body is `overflow:
 *  hidden` under an app, and the app scrolls inside its own document. */
export function contentViewKey(active: string | null, overlay: PanelOverlay): string | null {
  if (overlay) {
    if (overlay.type === 'form') return `form:${inlineFormKey(overlay.form)}`;
    if (overlay.type === 'file-preview') return `file:${overlay.path}`;
    if (overlay.type === 'url-preview') return `url:${overlay.url}`;
    if (overlay.type === 'notification-detail') return `notification:${overlay.notification.id}`;
    return overlay.type;
  }
  return active;
}
