import { releaseCheck, showToast } from '../../store/store';
import { setReleaseCheckConfig, type ReleaseCheck } from '../../api/client/control';
import { useServerBackedField } from '../../hooks/useServerBackedField';
import { errorDetail } from '../../utils/errorDetail';

/** Write the *update address* through the gateway, which answers with the
 *  fresh release check. `''` goes back to the published release. */
async function saveUpdateAddress(address: string): Promise<void> {
  try {
    releaseCheck.value = await setReleaseCheckConfig({ update_address: address });
  } catch (e) {
    showToast(`Couldn't save the update address: ${errorDetail(e)}`, 'error');
  }
}

/** The notice that an update address is in force, or refused. It must be
 *  impossible to miss while set, since every update check then reads somewhere
 *  other than the published release. Nothing renders when unset. */
export function UpdateAddressBanner({ check }: { check: ReleaseCheck | null }) {
  const address = check?.update_address ?? null;
  if (address === null) return null;
  const useDefault = (
    <>
      {' '}
      <button class="accent-link" onClick={() => { void saveUpdateAddress(''); }}>
        Use default
      </button>
    </>
  );
  if (check?.update_address_error) {
    return (
      <div class="system-notice" data-role="update-address-banner">
        <span class="update-address-flag">Update address refused.</span>{' '}
        {check.update_address_error}. Update checks fail until you fix it.
        {useDefault}
      </div>
    );
  }
  return (
    <div class="system-notice" data-role="update-address-banner">
      <span class="update-address-flag">Update address overridden.</span>{' '}
      Updates come from <code>{address}</code>, not the published release.
      {useDefault}
    </div>
  );
}

/** The editor for the update address. Keyed on the stored value by its caller,
 *  so a save, or a change from the CLI, redraws it from the gateway. */
export function UpdateAddressRow({ stored }: { stored: string }) {
  const [draft, setDraft] = useServerBackedField(stored);
  const changed = draft.trim() !== stored;
  return (
    <div class="settings-row update-address-row" data-search-anchor="system:update-address">
      <span class="settings-row-label">Update address</span>
      <div class="update-address-controls">
        <input
          class="settings-text-input update-address-input"
          type="url"
          value={draft}
          placeholder="https://example.com/latest.json"
          spellcheck={false}
          onInput={(e) => setDraft((e.currentTarget as HTMLInputElement).value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && changed) void saveUpdateAddress(draft.trim());
          }}
        />
        <button
          class="action-btn"
          disabled={!changed}
          onClick={() => { void saveUpdateAddress(draft.trim()); }}
        >
          Save
        </button>
      </div>
      <div class="settings-row-note">
        A latest.json URL: https, or http to localhost. Empty uses the published
        release.
      </div>
    </div>
  );
}
