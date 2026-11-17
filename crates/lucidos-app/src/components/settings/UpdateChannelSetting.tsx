import { useState } from 'preact/hooks';
import { releaseCheck, showToast } from '../../store/store';
import { setReleaseCheckConfig, type ReleaseCheck, type ReleaseCheckConfig } from '../../api/client/control';
import { errorDetail } from '../../utils/errorDetail';
import { hasUpdateChannelChoice, updateChannelLabel } from '../../utils/updateChannel';
import { Explainer } from '../shared/Explainer';
import { Disclosure } from '../shared/Disclosure';
import { DisclosureChevron } from '../shared/DisclosureChevron';
import { ArrivalList } from '../shared/ArrivalList';
import { useArrivals } from '../shared/arrivals';

async function saveChannel(config: ReleaseCheckConfig): Promise<boolean> {
  try {
    releaseCheck.value = await setReleaseCheckConfig(config);
    return true;
  } catch (e) {
    showToast(`Couldn't save the update channel: ${errorDetail(e)}`, 'error');
    return false;
  }
}

/** Only exceptional states draw a notice. The picker itself names the channel. */
export function UpdateChannelNotices({ check }: { check: ReleaseCheck | null }) {
  if (!check) return null;
  const error = check.channel_error ?? check.update_address_error ?? check.last_error;
  if (error) return <div class="system-notice" data-role="update-channel-error">
    Update check failed{check.channel ? ` on ${updateChannelLabel(check.channel)}` : ''}: {error}.{' '}
    {check.channel && check.channel !== 'stable' && <button class="accent-link" onClick={() => { void saveChannel({ channel: 'stable' }); }}>Back to Stable</button>}
  </div>;
  if (!check.ahead_of_channel || !check.channel_version || !check.channel) return null;
  return <div class="system-notice" data-role="update-channel-ahead">
    You’re ahead of {updateChannelLabel(check.channel)}: running {check.current_version}, channel {check.channel_version}. Your version is kept.
  </div>;
}

const explanations: Record<string, string> = {
  stable: 'Published releases. Switching back keeps your installed version and every saved custom address.',
  beta: 'Release candidates that passed the install checks. A newer version installs only when you choose it.',
  local: 'The signed candidate staged on this machine. The address follows the local gateway, even when its port changes.',
};

export function UpdateChannelSetting({ check }: { check: ReleaseCheck }) {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState('');
  const [url, setUrl] = useState('');
  const [saving, setSaving] = useState(false);
  const slots = check.custom ?? [];
  const arrived = useArrivals(slots.map(slot => slot.name));
  if (!hasUpdateChannelChoice(check)) return null;
  const change = async (config: ReleaseCheckConfig): Promise<boolean> => {
    setSaving(true);
    try { return await saveChannel(config); } finally { setSaving(false); }
  };
  const add = async () => {
    if (await change({ custom_upsert: { name: name.trim(), url: url.trim() } })) {
      setName(''); setUrl('');
    }
  };
  return <div data-search-anchor="system:update-channel">
    <div class="settings-row">
      <span class="settings-row-label">Update channel</span>
      <div class="segmented-control" role="group" aria-label="Update channel">
        {['stable', 'beta', 'local'].filter(channel => check.channels?.some(c => c.channel === channel && c.available)).map(channel =>
          <div class={`update-channel-segment${check.channel === channel ? ' active' : ''}`} key={channel}>
            <button class={`segmented-btn${check.channel === channel ? ' active' : ''}`} aria-pressed={check.channel === channel} disabled={saving}
              onClick={() => { void change({ channel }); }}>{updateChannelLabel(channel)}</button>
            <Explainer title={updateChannelLabel(channel)}><p>{explanations[channel]}</p></Explainer>
          </div>)}
      </div>
    </div>
    <div class="update-channel-fold">
      <button type="button" class="settings-disclosure-toggle" aria-expanded={open} onClick={() => setOpen(!open)}>
        <DisclosureChevron open={open} />Custom addresses ({slots.length}){check.channel?.startsWith('custom:') ? ` · ${check.channel.slice(7)}` : ''}
      </button>
      <Explainer title="Custom addresses"><p>Save named latest.json addresses, then select one for update checks. Removing the selected address returns to Stable.</p><p>Addresses use HTTPS, or HTTP to localhost. Every bundle must pass signature verification. Switching channels never downgrades Lucidos.</p></Explainer>
    </div>
    <Disclosure open={open}>
      <ArrivalList items={slots} keyOf={slot => slot.name} arrived={arrived}>
        {(slot, marker) => <div class={`settings-row${marker ? ` ${marker}` : ''}`}>
          <span class="settings-row-label">{slot.name}<br /><span class="update-channel-url">{slot.url}</span></span>
          <button class="action-btn" aria-pressed={check.channel === `custom:${slot.name}`} disabled={saving || check.channel === `custom:${slot.name}`}
            onClick={() => { void change({ channel: `custom:${slot.name}` }); }}>{check.channel === `custom:${slot.name}` ? 'Selected' : 'Select'}</button>
          <button class="action-btn" disabled={saving} aria-label={`Remove ${slot.name}`} onClick={() => { void change({ custom_remove: slot.name, ...(check.channel === `custom:${slot.name}` ? { channel: 'stable' } : {}) }); }}>Remove</button>
        </div>}
      </ArrivalList>
      <form class="settings-row" onSubmit={(e) => { e.preventDefault(); if (!saving && name.trim() && url.trim()) void add(); }}>
        <span class="settings-row-label">Add address<Explainer title="Add address"><p>Use a unique name with letters, numbers, underscores or hyphens, and a latest.json URL. Adding an address saves it without selecting it.</p></Explainer></span>
        <div class="update-channel-fields">
          <input class="settings-text-input" aria-label="Name" placeholder="Name, e.g. staging" value={name} disabled={saving} onInput={e => setName(e.currentTarget.value)} />
          <input class="settings-text-input" type="url" aria-label="Address" placeholder="https://example.com/latest.json" spellcheck={false} value={url} disabled={saving} onInput={e => setUrl(e.currentTarget.value)} />
          <button class="action-btn" type="submit" disabled={saving || !name.trim() || !url.trim()}>Add address</button>
        </div>
      </form>
    </Disclosure>
  </div>;
}
