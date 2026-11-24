import type { MainCheckSetting } from '../../api/client';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { currentMainCheckSetting, setMainCheckSetting } from '../../store/actions/preferences';
import { mainCheck, preferences } from '../../store/store';
import { Dropdown, DropdownSkeleton, type DropdownOption } from '../shared/Dropdown';
import { Explainer } from '../shared/Explainer';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';

export function mainCheckSettingOptions(): DropdownOption[] {
  return [
    { value: 'here', label: 'Here', description: 'This workspace checks main and repairs it' },
    { value: 'another-host', label: 'Another host', description: 'Another workspace checks main' },
    { value: 'off', label: 'Off', description: '/harden runs every suite itself' },
  ];
}

export function shownMainCheckSetting(
  stored: MainCheckSetting | null,
  resolved: MainCheckSetting | undefined,
): MainCheckSetting | null {
  return stored ?? resolved ?? null;
}

export function MainCheckSettingSection() {
  const prefs = preferences.value;
  const setting =
    prefs.status === 'loaded'
      ? shownMainCheckSetting(currentMainCheckSetting(), mainCheck.value.setting)
      : null;
  const showSkeleton = useDelayedFlag(setting === null && prefs.status !== 'failed');

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="coding-agents:main-check">
        Main check
        <Explainer title="Main check">
          <p>
            After main moves, the main check runs the slow suites on GitHub: the full engine
            suite, the crate and script suites on Linux and macOS, and every browser spec. A red
            main check opens a repair thread.
          </p>
          <p>
            While a main check covers this workspace, /harden runs only the fast suites and
            leaves the slow ones to it. With the setting off, /harden runs every suite before a
            change lands.
          </p>
          <p>
            Two workspaces set to Here both check main and both repair a red one. Set one of them
            to Another host.
          </p>
        </Explainer>
      </div>
      {prefs.status === 'failed' && <LoadableError noun="preferences" error={prefs.error} />}
      <div class="list-rows">
        <div class="list-row">
          <div class="list-row-info">
            <div class="title">Run the main check</div>
            <div class="list-row-details list-row-details-prose">
              Only one workspace should check main. Two set to Here both repair it.
            </div>
          </div>
          <div class="list-row-actions">
            <LoadingFade class="dropdown-slot" showSkeleton={showSkeleton} skeleton={<DropdownSkeleton w="7rem" />}>
              {setting !== null ? (
                <Dropdown
                  options={mainCheckSettingOptions()}
                  value={setting}
                  onChange={(v) => void setMainCheckSetting(v as MainCheckSetting)}
                />
              ) : null}
            </LoadingFade>
          </div>
        </div>
      </div>
    </div>
  );
}
