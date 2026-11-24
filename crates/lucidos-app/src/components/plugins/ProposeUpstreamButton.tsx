import { useState } from 'preact/hooks';
import { proposePluginUpstreamAction } from '../../store/actions/plugin-install';

export function ProposeUpstreamButton(
  { pluginId, pluginName }: { pluginId: string; pluginName: string },
) {
  const [busy, setBusy] = useState(false);
  return (
    <button
      type="button"
      class="action-btn action-btn-secondary"
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
      Send changes to author
    </button>
  );
}
