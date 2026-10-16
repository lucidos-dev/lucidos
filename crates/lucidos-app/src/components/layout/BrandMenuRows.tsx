/**
 * The Lucidos menu's rows: everything the brand menu draws inside its panel.
 *
 * A chunk of its own, fetched once the boot splash lifts (ADR 0288). The menu
 * opens only once it is in memory, so no row pops in (ADR 0436).
 */

import { connectionStatus, visibleWorkspaceName, searchEverywhereOpen, searchEverywhereAnchor, llmConfigured, lucidosRelease, lucidosReleaseDirty, whatsNewSeenRelease } from '../../store/store';
import type { ConnectionStatus } from '../../store/types';
import { hasUnreadWhatsNew } from '../../store/actions/whatsNew';
import { unfocusThread } from '../../store/actions/threads';
import { homeThreadId } from '../../store/actions/homeThread';
import { openHome } from '../../store/actions/dashboardNav';
import { openWhatsNew } from '../../store/actions/menu';
import { connectionNotice } from '../../utils/connectionNotice';
import { lucidosVersionLabel, lucidosVersionTooltip } from '../../utils/lucidosVersion';
import { composeHandlers } from '../chat/composeHandlers';
import { focusSearchInput } from '../search/searchEverywhereActions';
import { ComposeIcon, SearchIcon, HelpIcon, HomeIcon, LucidosMarkIcon } from '../shared/icons';
import { confirmAndStartSetupInterview } from '../shared/setupInterview';
import { NotificationsMenuGroup } from './NotificationsMenuRows';
import { ActivityMenuGroup } from './ActivityMenuRows';
import { WorkspaceRefreshRow, WorkspaceRestartRow } from './WorkspaceMenuRows';
import { WorkspacesMenuRow } from './WorkspaceSwitcher';

/** The notice at the head of the panel: a statement, not a row.
 *
 *  `role="none"`, as `.brand-menu-confirm-row` takes it. The panel is a
 *  `role="menu"`, which expects menuitems, groups and separators. An announced
 *  node here would be an orphan the keyboard roving has to step past.
 *
 *  Nothing is lost by it. The state is already in the opener's accessible name
 *  (`BrandMenuButton`'s `label`), where a screen reader meets it first. This
 *  notice is for a phone, which has no hover and no tooltip.
 *
 *  Pure, so `vnodeToText` can flatten it with no DOM: which element it becomes
 *  and which classes it carries IS this surface's behaviour, the same way it is
 *  for the workspace rows.
 *
 *  The SHORT detail. The panel is `--header-surface-width` wide, so the explainer
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

/** The Lucidos menu's Home row, with its own separator, so it leaves nothing
 *  behind while the home thread is off. No thread list draws the home thread.
 *  So this row is the way to it from the phone's thread drawer, whose header
 *  has no Home icon. A plain click rather than `composeHandlers`: opening Home
 *  is for reading, so it raises no keyboard. */
export function HomeMenuGroup({ onClose }: { onClose: () => void }) {
  if (!homeThreadId.value) return null;
  return (
    <>
      <button
        type="button"
        class="brand-menu-item home-thread-btn"
        role="menuitem"
        onClick={() => { onClose(); openHome(); }}
      >
        <HomeIcon />
        Home
      </button>
      <div class="brand-menu-separator" role="separator" />
    </>
  );
}

/** The rows, in order. `LucidosMenu` in `HeaderMark.tsx` documents the menu
 *  and the two flags. */
export function BrandMenuRows({ onClose, actionsInRow, homeInRow }: {
  onClose: () => void;
  actionsInRow: boolean;
  homeInRow: boolean;
}) {
  // The version of LUCIDOS the user is running: the umbrella release, plus a
  // `*` when the code has moved past it. One number, the same on every
  // platform. The engine's CalVer and the two build ids are parts, not the
  // product, so they live in Settings > System. Always a string, so the row
  // never changes shape. See `utils/lucidosVersion.ts`.
  const release = lucidosRelease.value;
  const releaseDirty = lucidosReleaseDirty.value;
  const version = lucidosVersionLabel(release, releaseDirty);
  const versionTooltip = lucidosVersionTooltip(release, releaseDirty);
  // Is there a release whose notes this client has not read? The dot shows it.
  // The sentence below says it to a screen reader and a desktop hover, since
  // a bare dot says nothing there. `hasUnreadWhatsNew` is false while the
  // release is unknown, so `versionTooltip` is defined whenever this composes.
  const unread = hasUnreadWhatsNew(release, whatsNewSeenRelease.value);
  const versionLabel = unread ? `${versionTooltip} · new release notes` : versionTooltip;
  // Every menu action closes the menu first. `composeHandlers` focuses the
  // prompt inside the touch gesture, the only way iOS raises the keyboard.
  // It runs the action AFTER the focus, since a signal re-render between the
  // touch and the focus expires the gesture.
  const newThread = composeHandlers(() => {
    onClose();
    unfocusThread();
  });

  const searchEverywhere = composeHandlers(
    () => {
      onClose();
      // NO anchor. The "never anchor={null}" rule covers a TOGGLE-opened
      // overlay, whose re-tap must close it rather than race the dismiss.
      // Nothing toggles the palette here: a menu row that unmounts opens it,
      // and the desktop button still sets itself.
      //
      // Naming the mark would be wrong. An anchor is exempt from the dismiss,
      // so a tap on the mark would leave the palette open. The mark's own
      // handler would then open the menu ON TOP of it. Left null, that tap
      // dismisses the palette and the paired swallow keeps the menu shut.
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
    // No focus nudge, matching the header button. The next thing on screen is
    // a confirm modal. Focusing the prompt would raise the iOS keyboard behind
    // it and leave it up if the user backs out. The wrapper stays for its
    // touch and click dedup.
    () => {},
  );
  return (
    <>
      {/* Why the mark is dim, in words, and nothing at all while it is lit.
          It goes ABOVE the identity row because it is news of a different
          kind: that row says what you are running and what is new in it, and
          this says that right now you are not reaching it at all. Read from
          inside the panel, so a drop while the menu is open writes the notice
          in under the user's eyes rather than waiting for the next open. */}
      {connectionNoticeRow(connectionStatus.value, visibleWorkspaceName.value)}

      {/* The first row from the thread drawer's mark. Both thread pane rows
          carry a Home icon instead (`ThreadHeaderHomeButton`). */}
      {!homeInRow && <HomeMenuGroup onClose={onClose} />}

      {/* The work in flight, one row per job, which is what the spinning
          badge on the mark advertises. Above the notifications because it is
          what is happening NOW, and the only place its progress is told.
          Renders nothing when nothing runs. */}
      <ActivityMenuGroup onClose={onClose} />

      {/* Where the unread notifications are, this workspace included. It
          leads the panel's news because it is what the mark's badge just
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

      {/* Three of the actions the desktop header carries as icons; Home,
          the fourth, has its own rule above. Repeating
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
    </>
  );
}
