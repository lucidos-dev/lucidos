import { useSignal } from '@preact/signals';
import { restartRequired, engineVersionReady, updateAvailable, engineNewVersionReady } from '../../store/store';
import { initiateEngineRestart } from '../../store/actions/chat-changes';
import { refreshClient } from '../../hooks/sw-update';
import { ReloadIcon, PowerIcon } from '../shared/icons';

/** Display state for the Refresh row: its tooltip and whether to show the
 *  client-update dot. Pure so the whole presentation rule is unit-testable from
 *  the raw signals.
 *
 *  - `clientUpdateAvailable` = a newer client bundle is served (the
 *    `updateAvailable` build-id signal; named without the shadow so the body
 *    can't confuse it for the imported signal).
 *  - `enginePending` = an engine switch is pending or building
 *    (`restartRequired || engineVersionReady`).
 *
 *  The dot is advertised only when a newer bundle exists AND no engine switch is
 *  pending/building. With the engine serving a build-pinned client
 *  (api/frontend_snapshot.rs) the web `updateAvailable` signal is already false
 *  during a pending switch (the served build matches the loaded one), so
 *  `!enginePending` is defense-in-depth here, and it also holds the Tauri
 *  app-version axis (which sets `updateAvailable` directly) until after the
 *  switch, so this actionable control never invites a refresh onto the still-old
 *  engine. */
export function refreshRowState(
  clientUpdateAvailable: boolean,
  enginePending: boolean,
): { tooltip: string; showUpdateBadge: boolean } {
  const update = clientUpdateAvailable && !enginePending;
  const base = 'Reload the client';
  return { tooltip: update ? `Update available · ${base}` : base, showUpdateBadge: update };
}

/** Display state for the Restart row. `ready` is the honest "there is something
 *  to switch onto" signal (`engineNewVersionReady()`), NOT the apply-time
 *  `restartRequired`: under the background-build scheme the dev binary is still
 *  compiling right after Apply, so keying off `restartRequired` would light the
 *  row before there was anything to switch to.
 *
 *  Three outputs from the one predicate, because the row has to say it three
 *  ways: `tooltip` is the hover text and the accessible name, `pending` lights
 *  the glyph, and `badge` is the visible words. The badge is what makes the
 *  state legible: a tinted power glyph reads as this row's own colour, not as
 *  news, and the tooltip that spelled it out is desktop-hover-only, so on a
 *  phone the whole announcement was one blue icon. */
export function restartRowState(
  ready: boolean,
): { tooltip: string; pending: boolean; badge: string | null } {
  return {
    tooltip: ready ? 'Restart onto the new version' : 'Restart this workspace',
    pending: ready,
    badge: ready ? 'New version' : null,
  };
}

/**
 * Refresh: reload the client. A plain row with a plain tap.
 *
 * It used to be half of one control, where a HOLD on this row revealed the
 * restart confirm. Two rows say what they do; a hold does not, and there was
 * nothing in the menu to discover it from.
 */
export function WorkspaceRefreshRow({ onClose }: { onClose: () => void }) {
  const enginePending = restartRequired.value || engineVersionReady.value;
  const { tooltip, showUpdateBadge } = refreshRowState(updateAvailable.value, enginePending);

  return (
    <button
      type="button"
      class="brand-menu-item brand-menu-refresh"
      role="menuitem"
      aria-label={tooltip}
      data-tooltip={tooltip}
      onClick={() => { onClose(); refreshClient(); }}
    >
      <ReloadIcon />
      Refresh
      {/* Non-interactive (pointer-events: none in CSS) so it never intercepts
          the tap the row itself owns. */}
      {showUpdateBadge && <span class="brand-menu-refresh-badge" aria-hidden="true" />}
    </button>
  );
}

/**
 * Restart: stop and start this workspace's engine, which is also how a new
 * version is switched onto.
 *
 * A tap does NOT restart. It turns the row into its own confirmation, an `OK`
 * button in the trailing slot the Workspaces row puts its value pill in, and
 * only that button fires. Restarting is disruptive (every running session is
 * torn down and resumed) and it sits directly under a Refresh that is not, so a
 * mis-tap must not be able to do it.
 *
 * The confirm is rendered INSIDE the menu panel on purpose: pressing it neither
 * dismisses the menu nor gets swallowed by the outside-click contract, which a
 * global confirm modal would hit (the menu would treat it as "outside" and eat
 * the click). There is no Cancel: closing the menu IS the cancel, it costs
 * nothing to reach (tap anywhere outside, or Escape), and this state lives in a
 * component the Overlay unmounts on close, so backing out genuinely resets the
 * prompt rather than leaving it armed for the next open. It also keeps the row
 * inside the panel's fixed width, which an icon plus the label plus two buttons
 * did not fit in.
 *
 * `onClose` shuts the menu once a restart is actually initiated: the app is
 * about to reconnect, so leaving the menu sitting open over it is stale chrome.
 */
export function WorkspaceRestartRow({ onClose }: { onClose: () => void }) {
  const confirming = useSignal(false);
  const { tooltip, pending, badge } = restartRowState(engineNewVersionReady());

  if (confirming.value) {
    // `role="none"`, so this is not an orphan node in a `role="menu"` panel:
    // while it is showing, the row is a prompt with a button in it, not a menu
    // item.
    return (
      <div class={`brand-menu-item brand-menu-confirm-row${pending ? ' is-pending' : ''}`} role="none">
        <PowerIcon />
        Restart
        <span class="brand-menu-confirm-actions">
          <button
            type="button"
            class="brand-menu-confirm"
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              confirming.value = false;
              onClose();
              void initiateEngineRestart();
            }}
          >
            OK
          </button>
        </span>
      </div>
    );
  }

  return (
    <button
      type="button"
      class={`brand-menu-item${pending ? ' is-pending' : ''}`}
      role="menuitem"
      aria-label={tooltip}
      data-tooltip={tooltip}
      onClick={() => { confirming.value = true; }}
    >
      <PowerIcon />
      Restart
      {/* The same pill the Workspaces and Lucidos rows carry, in the same
          trailing slot, so a row's own status is always found in one place. It
          is absent from the confirm state above because that slot is where the
          OK goes, and the lit glyph carries the state through the two taps. */}
      {badge && (
        <span class="brand-menu-value brand-menu-restart-badge">
          <span class="brand-menu-value-name">{badge}</span>
        </span>
      )}
    </button>
  );
}
