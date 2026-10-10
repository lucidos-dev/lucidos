import { useState } from 'preact/hooks';
import { proposePluginUpstreamAction } from '../../store/actions/plugin-install';

/** Offer the user's local patch to the plugin's author.
 *
 *  Shown wherever a kept local patch is visible: on the install receipt right
 *  after an update that merged one, and on the Plugins row whenever the
 *  Modified badge is up. The receipt is timely but scrolls away; the row is
 *  where the offer stays reachable.
 *
 *  The click only asks the engine for a patch and a thread. Everything about
 *  forks and pull requests happens in that thread. */
export function ProposeUpstreamButton(
  { pluginId, pluginName }: { pluginId: string; pluginName: string },
) {
  const [busy, setBusy] = useState(false);
  return (
    <button
      type="button"
      class="action-btn"
      disabled={busy}
      onClick={async () => {
        setBusy(true);
        try {
          await proposePluginUpstreamAction(pluginId, pluginName);
        } finally {
          setBusy(false);
        }
      }}
    >
      Propose upstream
    </button>
  );
}
