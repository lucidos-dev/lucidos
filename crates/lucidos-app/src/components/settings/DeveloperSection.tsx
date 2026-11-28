import { showConfirm } from '../../store/store';
import { currentTerminalEnabled, setTerminalEnabled } from '../../store/actions/preferences';
import { Explainer } from '../shared/Explainer';

export const TERMINAL_WARNING =
  "Lets every paired device run commands on this computer, with your account's full access. "
  + 'Only turn it on if you trust your devices and your apps.';

async function changeTerminal(enabled: boolean): Promise<void> {
  if (enabled && !(await showConfirm(TERMINAL_WARNING, 'Turn on', { variant: 'default', title: 'Turn on Terminal?' }))) {
    return;
  }
  await setTerminalEnabled(enabled);
}

export function DeveloperSection() {
  const enabled = currentTerminalEnabled();
  return (
    <div class="settings-section">
      <div class="settings-section-title">Developer</div>
      <div class="settings-row" data-search-anchor="developer:terminal">
        <span class="settings-row-label">
          Terminal
          <Explainer title="Terminal">
            <p>
              Gives every thread a shell on this computer. A coding-agent thread's shell starts in
              its worktree, and a chat thread's in the workspace's data folder.
            </p>
            <p>{TERMINAL_WARNING}</p>
            <p>
              An app opened in its own browser tab could turn this on and use it too. Agents cannot
              turn it on, and they never see what the terminal shows.
            </p>
            <p>Turning it off ends every running shell.</p>
            <p>
              The terminal needs a direct connection to this workspace's own gateway. A workspace
              opened through another gateway shows no terminal.
            </p>
          </Explainer>
        </span>
        <label class="toggle-switch">
          <input
            type="checkbox"
            aria-label="Terminal"
            checked={enabled}
            onChange={(e) => {
              const input = e.currentTarget as HTMLInputElement;
              const wanted = input.checked;
              input.checked = enabled;
              void changeTerminal(wanted);
            }}
          />
          <span class="toggle-slider" />
        </label>
      </div>
    </div>
  );
}
