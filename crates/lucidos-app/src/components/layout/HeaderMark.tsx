import { useState } from 'preact/hooks';
import { createPortal } from 'preact/compat';
import { connectionStatus, visibleWorkspaceName, searchEverywhereOpen, searchEverywhereAnchor, llmConfigured, lucidosRelease, lucidosReleaseDirty, whatsNewSeenRelease } from '../../store/store';
import type { ConnectionStatus } from '../../store/types';
import { hasUnreadWhatsNew } from '../../store/actions/whatsNew';
import { unfocusThread } from '../../store/actions/threads';
import { openWhatsNew } from '../../store/actions/menu';
import { crossWorkspaceUnreadTotal, refreshOtherWorkspacesUnread } from '../../store/actions/app-badge';
import { connectionNotice, connectionNoticeSentence, connectionPhrase } from '../../utils/connectionNotice';
import { lucidosVersionLabel, lucidosVersionTooltip } from '../../utils/lucidosVersion';
import { composeHandlers } from '../chat/promptFocus';
import { focusSearchInput } from '../search/searchEverywhereActions';
import { Overlay } from '../shared/Overlay';
import { ComposeIcon, SearchIcon, HelpIcon, LucidosMarkIcon } from '../shared/icons';
import { confirmAndStartSetupInterview } from '../shared/setupInterview';
import { BrandBadge, UnreadBrandBadge, unreadBadgeLabel } from './BrandBadge';
import { NotificationsMenuGroup } from './NotificationsMenuRows';
import { WorkspaceRefreshRow, WorkspaceRestartRow } from './WorkspaceMenuRows';
import { WorkspacesMenuRow } from './WorkspaceSwitcher';

/** The notice at the head of the panel: a statement, not a row.
 *
 *  `role="none"` for the reason `.brand-menu-confirm-row` takes it: the panel is
 *  a `role="menu"`, whose children are expected to be menuitems, groups and
 *  separators, so an announced node here would be an orphan the keyboard roving
 *  has to step past. Nothing is lost by it. The state is already in the
 *  accessible name of the control that opened the panel (see `BrandMenuButton`'s
 *  `label`), which is where a screen reader meets it first and without opening
 *  anything; this notice is the half that a phone, which has no hover and no
 *  tooltip, could not reach at all.
 *
 *  Pure, so `vnodeToText` can flatten it with no DOM: which element it becomes
 *  and which classes it carries IS this surface's behaviour, the same way it is
 *  for the workspace rows.
 *
 *  The SHORT detail. The panel is `--brand-menu-width` wide, so the explainer
 *  wrapped to three lines and pushed every row below it down. The connection
 *  bar states the full sentence, on screen and without a tap. */
export function connectionNoticeRow(status: ConnectionStatus, workspace: string | null) {
  const notice = connectionNotice(status, workspace, 'short');
  if (!notice) return null;
  return (
    <div class={`brand-menu-notice brand-menu-notice-${status}`} role="none">
      <span class={`status-dot ${status}`} aria-hidden="true" />
      <span class="brand-menu-notice-text">
        <b>{notice.title}</b>{' '}{notice.detail}
      </span>
    </div>
  );
}

/**
 * The Lucidos menu: New thread, Search everywhere, Workspaces, Refresh.
 *
 * A CENTRED MODAL over a dimmed app, not a panel hanging off whatever opened
 * it. Three hosts open it (the mobile thread pane's centred mark, the mobile
 * threads pane's row mark, the desktop thread pane's mark) and an anchored panel
 * put it somewhere different for each; centred, there is one place to look. It
 * is also what lets the mark keep its position in a fixed-width cluster without
 * the menu inheriting that constraint. Centred on the THREAD PANE where there is
 * one (desktop), on the viewport where the pane is the whole screen (mobile):
 * see `.brand-menu` in styles/header-mark.css.
 *
 * Every host keeps its OWN open state and passes its own element as `anchor`.
 * An `anchor={null}` toggle-opened overlay is a known bug shape: on touch the
 * outside-pointerdown dismiss closes it and the toggle's own `touchend`
 * immediately reopens it, so it never closes. See `.claude/rules/frontend.md`
 * § Modals & Popovers.
 */
function LucidosMenu({ open, onClose, anchor, actionsInRow }: {
  open: boolean;
  onClose: () => void;
  anchor: HTMLElement | null;
  /** The host's own header row already carries New thread, Search everywhere
   *  and Setup interview as icons, so the menu does not repeat them. True on
   *  desktop only: no mobile header has room for those three, which is why they
   *  came into the menu in the first place. What is left is Workspaces and
   *  Refresh, the pair the retired workspace switcher owned and the only part of
   *  this menu desktop can reach nowhere else. */
  actionsInRow: boolean;
}) {
  // The version of LUCIDOS the user is running: the umbrella release, plus a
  // `*` when the code has moved past it. One number, the same one on every
  // platform. The engine's CalVer, this client's build id and the service
  // worker's are all parts rather than the product, and they belong in Settings
  // > System, where they are labelled apart. Always a string, so the row never
  // changes shape. See `utils/lucidosVersion.ts`.
  const release = lucidosRelease.value;
  const releaseDirty = lucidosReleaseDirty.value;
  const version = lucidosVersionLabel(release, releaseDirty);
  const versionTooltip = lucidosVersionTooltip(release, releaseDirty);
  // Is there a release whose notes this client has not read? The dot is the
  // visual half of that; the sentence below is the half a screen reader and a
  // desktop hover get, because an unexplained dot says nothing on either
  // surface. `hasUnreadWhatsNew` is false while the release is unknown, so
  // `versionTooltip` is never undefined when this composes.
  const unread = hasUnreadWhatsNew(release, whatsNewSeenRelease.value);
  const versionLabel = unread ? `${versionTooltip} · new release notes` : versionTooltip;
  // Every menu action closes the menu first. `composeHandlers` focuses the
  // prompt from inside the touch gesture, which is the only way iOS raises the
  // keyboard, and it runs the action AFTER the focus for the same reason: a
  // signal re-render between the touch and the focus expires the gesture.
  const newThread = composeHandlers(() => {
    onClose();
    unfocusThread();
  });

  const searchEverywhere = composeHandlers(
    () => {
      onClose();
      // NO anchor. The "never anchor={null}" rule is about a TOGGLE-opened
      // overlay, where the anchor exemption is what lets a re-tap on the toggle
      // close it instead of racing the outside-dismiss. Nothing toggles the
      // palette here: it is opened from a menu row that unmounts immediately,
      // and the desktop button still sets itself.
      //
      // Naming the mark instead would be actively wrong. The anchor is exempt
      // from the palette's outside-dismiss, so a tap on the mark would not
      // close the palette, and the mark's own handler would open the menu ON
      // TOP of it. Left null, that tap dismisses the palette and the paired
      // swallow keeps the menu shut, which is what a tap outside should do.
      searchEverywhereAnchor.value = null;
      searchEverywhereOpen.value = true;
    },
    focusSearchInput,
  );

  const setupInterview = composeHandlers(
    () => {
      onClose();
      void confirmAndStartSetupInterview();
    },
    // No focus nudge, matching the header button: the next thing on screen is a
    // confirm modal, so focusing the prompt would raise the iOS keyboard behind
    // it and leave it up if the user backs out. The touch/click dedup is what
    // this wrapper is still here for.
    () => {},
  );

  return (
    <>
      {/* The dim, portaled separately from the panel and NOT as an <Overlay>
          backdrop: `backdrop` mode renders the scrim as the panel's own
          container, which `Overlay` only ever renders inline, and inline is
          exactly where a `position: fixed` box cannot be trusted here (the
          transformed `.header-nav-cluster` ancestor). Two portaled siblings
          sidestep that. */}
      {open && typeof document !== 'undefined' && createPortal(
        <div class="brand-menu-scrim" aria-hidden="true" />,
        document.body,
      )}

      {/* Centred from CSS, so no measure pass and no hidden first frame: the
          panel is `position: fixed` and portaled to <body> so its offsets
          resolve against the viewport. */}
      {/* `role="menu"`, not `dialog`: the toggle advertises `aria-haspopup="menu"`
          and every row is a `menuitem`, and a `menuitem` must be owned by a
          menu-role container to mean anything. Centred with a scrim is a
          PLACEMENT; it does not make this a dialog. */}
      <Overlay
        open={open}
        onClose={onClose}
        anchor={anchor}
        backdrop={false}
        portal
        panelClass="brand-menu"
        panelRole="menu"
        panelProps={{ 'aria-label': 'Lucidos menu' }}
      >
        {/* Why the mark is dim, in words, and nothing at all while it is lit.
            It goes ABOVE the identity row because it is news of a different
            kind: that row says what you are running and what is new in it, and
            this says that right now you are not reaching it at all. Read from
            inside the panel, so a drop while the menu is open writes the notice
            in under the user's eyes rather than waiting for the next open. */}
        {connectionNoticeRow(connectionStatus.value, visibleWorkspaceName.value)}

        {/* Where the unread notifications are, this workspace included. It
            LEADS the panel because it is the news the mark's badge just
            advertised, and the icon badge before that: a user who opened the
            menu because something said "1" must not have to hunt. Renders
            nothing, separator included, when everything is read. */}
        <NotificationsMenuGroup onClose={onClose} />

        {/* What you are running, and the answer to the question a version
            number raises. It leads the menu because it is identity rather than
            an action: the bar no longer says the word "Lucidos" anywhere, so
            this is where the product names itself and admits its version.
            Settings > System > What's New is the destination because that is
            what "0.26.3" means: the notes for the release, with every earlier
            one under it. System's Overview, with the other three versions and
            the update controls, is one tab along the subpanel switcher. */}
        <button
          type="button"
          class="brand-menu-item brand-menu-version"
          role="menuitem"
          // The pill's `*` is the whole of what says "this is not the published
          // release", and an asterisk read aloud, or hovered, says nothing. Both
          // surfaces get the sentence instead.
          aria-label={versionLabel}
          data-tooltip={versionLabel}
          // Names no release: this row is about the one you are RUNNING, which
          // is what the panel opens by itself. Going through `openWhatsNew`
          // anyway is what clears a target an update offer left behind.
          onClick={() => { onClose(); openWhatsNew(); }}
        >
          <LucidosMarkIcon />
          Lucidos
          <span class="brand-menu-value">
            {/* A release whose notes this client has not opened. Inside the
                pill, on the version it is about, rather than floating beside
                the row: it marks THAT number as new, and it goes away by being
                acted on. `aria-hidden` because a bare dot names nothing said
                aloud; the sentence in the row's label carries it instead. */}
            {unread && <span class="brand-menu-unread-dot" aria-hidden="true" />}
            <span class="brand-menu-value-name">{version}</span>
          </span>
        </button>
        {/* `role="separator"` is a real member of a menu's role set, so the row
            above reads as its own group rather than as a stray element the
            keyboard roving has to skip past. */}
        <div class="brand-menu-separator" role="separator" />

        {/* The three actions the desktop header carries as icons. Repeating
            them there would make the menu a second copy of the row above it,
            so they render only where the row has no room for them, which is
            both mobile headers. The *setup interview* is one of the three: its
            desktop icon (the `setup-interview` action in `threadHeaderActions`)
            is why mobile got a row at all. It is gated on a configured LLM for
            the same reason that icon is: the interview is a conversation, and
            there is nothing to have it with.

            The row is LABELLED "Setup guide" while the concept keeps its name
            everywhere else (the knowhow id, the artifact, the
            `SetupInterviewCompleted` event). That split is deliberate and
            user-facing only: "interview" reads as an interrogation on a menu
            row a newcomer meets before they know what it does. Grep either
            word and land here. */}
        {!actionsInRow && (
          <>
            <button type="button" class="brand-menu-item" role="menuitem" {...newThread}>
              <ComposeIcon />
              New thread
            </button>
            <button type="button" class="brand-menu-item" role="menuitem" {...searchEverywhere}>
              <SearchIcon />
              Search everywhere
            </button>
            {llmConfigured.value && (
              <button type="button" class="brand-menu-item" role="menuitem" {...setupInterview}>
                <HelpIcon />
                Setup guide
              </button>
            )}
          </>
        )}
        {/* Names the workspace you are in, and unfolds the list of the ones you
            could be in. It renders from inside the panel on purpose: the state
            it holds (unfolded or not, and the listing) belongs to one opening of
            the menu, and the Overlay unmounting on close is what resets it. */}
        <WorkspacesMenuRow onClose={onClose} />
        <WorkspaceRefreshRow onClose={onClose} />
        <WorkspaceRestartRow onClose={onClose} />
      </Overlay>
    </>
  );
}

/**
 * The Lucidos mark, doing three jobs at once: it is the brand, it is the
 * connection light, and it opens the menu.
 *
 * `placement` names the header row the mark sits on. That decides how it is
 * DRESSED, and one thing about what it opens: a row that already carries New
 * thread, Search everywhere and Setup interview as icons gets a menu that does
 * not repeat them (see `LucidosMenu`'s `actionsInRow`).
 *
 * - `cluster` (the mobile thread pane) is the centrepiece of a fixed-width nav
 *   cluster, at the larger tap target, and it is the connection light:
 *   `data-conn` drives the colour-and-motion ladder in styles/header-mark.css.
 * - `brand` (the desktop thread pane) is the same control on a row with room to
 *   spare, so it keeps the mark's own dressing and the connection light, adds
 *   the hover tooltip its neighbours in that bar all carry, and trims the menu.
 *   It replaced the `[Lucidos * workspace]` wordmark label; the workspace name
 *   beside it is now a plain label (`WorkspaceNameLabel`), not a second opener.
 * - `row` (the mobile threads pane) is DRESSED as a member of that row's icon
 *   run without sitting in it. It wears `.icon-btn.header-icon` so the run's
 *   BOX comes from the same rules its neighbours use, which is the only way to
 *   be sure it reads at their size; its colour departs from theirs, holding the
 *   bar's full foreground rather than the muted glyph colour, so the brand
 *   reads at the same strength on both panes (styles/header-mark.css). Where it
 *   SITS is the centred nav cluster's trailing edge, the column the other two
 *   rows give their forward chevron, so the mark does not move as the user
 *   swipes between panes (see MobileAppHeader). It carries NO `data-conn`: a
 *   glyph that dims itself among a row of icons that do not reads as disabled
 *   rather than as disconnected. One connection light, on the pane that owns
 *   it.
 */
export function BrandMenuButton({ placement = 'cluster' }: { placement?: 'cluster' | 'brand' | 'row' }) {
  const [anchor, setAnchor] = useState<HTMLElement | null>(null);
  const [open, setOpen] = useState(false);
  const status = connectionStatus.value;
  const inRow = placement === 'row';
  const onDesktop = placement === 'brand';
  // A middle dot rather than a comma between the two halves, matching the
  // separator every other multi-part tooltip in the bar uses (see
  // `brandBadgeTooltip`): the state is a second fact about the mark, not a
  // subordinate clause of the first.
  //
  // The degraded states spend the second half on the notice rather than on the
  // phrase alone. Naming the state is enough while everything works, and is the
  // least useful half of what we know once it does not. The SHORT detail: a
  // tooltip is read at a glance, and the bar under this row is already stating
  // the rest of it in full. Same table as the bar and the menu notice, so a
  // hover and a glance cannot disagree.
  const sentence = connectionNoticeSentence(status, visibleWorkspaceName.value, 'short');
  // The unread count is spoken HERE rather than on the badge that draws it.
  // That badge is `pointer-events: none` and aria-hidden. Neither a pointer nor
  // a screen reader reaches it; the mark is the element both land on. Without
  // this the count would be silent on the two panes the bell never appears on,
  // which is the whole reason it rides the mark.
  const unread = unreadBadgeLabel(crossWorkspaceUnreadTotal.value);
  const label = [
    'Lucidos menu',
    unread,
    inRow ? null : (sentence ?? connectionPhrase(status, visibleWorkspaceName.value)),
  ].filter(Boolean).join(' · ');

  return (
    <>
      {/* The badge is a SIBLING of the mark, not a child of it.
          `BrandBadge`'s busy state renders a real `<button>` (it opens the
          background-activity toast), and a button inside a button is invalid
          HTML: the inner one swallows the tap, so while the engine was
          building, tapping the mark opened the status toast instead of the
          menu. Desktop never hit this because its badge host is a `<span>`.
          The slot is the positioning context both share, and it is what puts
          the badge ON the mark's corner rather than in a flex slot beside it
          (styles/header-mark.css). Overlapping them means sibling markup is no
          longer the whole story, and the other two halves live in that
          stylesheet: the busy badge's hit area is reined in, since a square
          centred on the badge otherwise covers the mark, and the ready badge is
          click-through, since a span with no handler is still a hit target and
          a sibling gives its tap nothing to bubble to. */}
      <span class="brand-mark-slot">
        <button
          ref={setAnchor}
          type="button"
          class={inRow ? 'icon-btn header-icon brand-mark-row' : `brand-mark${open ? ' is-open' : ''}`}
          data-role="brand-menu-toggle"
          // Only the cluster mark is the connection light, so only it carries
          // the attribute the state rules key on (see the component doc).
          data-conn={inRow ? undefined : status}
          onClick={() => {
            // Re-read the peer counts on the way in, so the notifications group
            // is current rather than as of the last resume or tick.
            //
            // `WorkspacesMenuRow` deliberately hangs its own fetch off its
            // EXPAND instead, and that rule still holds for it: it has nothing
            // cached, so a fetch there would put Refresh and Restart behind a
            // spinner. The group does have a cache, renders from it instantly,
            // and this only corrects it.
            if (!open) void refreshOtherWorkspacesUnread();
            setOpen(!open);
          }}
          aria-haspopup="menu"
          aria-expanded={open}
          // Colour and motion carry the connection state visually on the cluster
          // mark; this is the half a screen reader gets. The row mark does not
          // show the state, so it does not claim to.
          aria-label={label}
          // The same sentence as a hover tooltip, and only on the desktop row:
          // there the mark took the place of a label that said "Lucidos" in
          // words, and every other control in that bar names itself on hover.
          // `data-tooltip` is a desktop-only surface anyway, so this is about
          // which ROW earns one, not about which viewport can show it.
          data-tooltip={onDesktop ? label : undefined}
        >
          {inRow ? <LucidosMarkIcon /> : <span class="brand-mark-glyph"><LucidosMarkIcon /></span>}
        </button>
        <BrandBadge />
        {/* The unread count, on the OPPOSITE corner from the state badge above
            so the two coexist. See `UnreadBrandBadge` for why it is the bottom
            one, and why it is not a button. */}
        <UnreadBrandBadge />
      </span>

      <LucidosMenu
        open={open}
        onClose={() => setOpen(false)}
        anchor={anchor}
        actionsInRow={onDesktop}
      />
    </>
  );
}
