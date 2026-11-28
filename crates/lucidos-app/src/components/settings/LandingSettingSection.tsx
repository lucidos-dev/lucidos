import type { LandingSetting } from '../../api/client';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { setLandingSetting } from '../../store/actions/preferences';
import { landingLane, preferences } from '../../store/store';
import { Dropdown, DropdownSkeleton, type DropdownOption } from '../shared/Dropdown';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';

const OPTIONS: DropdownOption[] = [
  { value: 'here', label: 'Here', description: 'This workspace lands changes on main' },
  { value: 'another-host', label: 'Another host', description: 'Send changes to the lander through origin' },
];

export function LandingSettingSection() {
  const prefs = preferences.value;
  const showSkeleton = useDelayedFlag(prefs.status !== 'loaded' && prefs.status !== 'failed');

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="coding-agents:landing">
        Landing
      </div>
      {prefs.status === 'failed' && <LoadableError noun="preferences" error={prefs.error} />}
      <div class="list-rows">
        <div class="list-row">
          <div class="list-row-info">
            <div class="title">Land changes</div>
            <div class="list-row-details list-row-details-prose">
              Set one workspace per repository to Here. Changes from the other workspaces wait for that lander.
            </div>
          </div>
          <div class="list-row-actions">
            <LoadingFade class="dropdown-slot" showSkeleton={showSkeleton} skeleton={<DropdownSkeleton w="7rem" />}>
              {prefs.status === 'loaded' && (
                <Dropdown
                  options={OPTIONS}
                  value={landingLane.value.setting ?? 'here'}
                  onChange={(value) => void setLandingSetting(value as LandingSetting)}
                />
              )}
            </LoadingFade>
          </div>
        </div>
      </div>
    </div>
  );
}
