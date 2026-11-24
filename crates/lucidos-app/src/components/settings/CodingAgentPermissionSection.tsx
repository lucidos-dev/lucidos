import { PREF_CODING_AGENT_CLAUDE_PERMISSION_MODE } from '@lucidos/preference-catalog';
import {
  CC_PERMISSION_MODES,
  currentCodingAgentPermissionMode,
  setCodingAgentPermissionMode,
  type CcPermissionMode,
} from '../../store/actions/preferences';
import { Dropdown, type DropdownOption } from '../shared/Dropdown';
import { Explainer } from '../shared/Explainer';


const PERMISSION_MODE_COPY: Record<CcPermissionMode, { label: string; description: string }> = {
  'accept-edits': {
    label: 'Accept edits',
    description: 'You approve anything outside the working directories',
  },
  auto: {
    label: 'Auto',
    description: "Claude Code's safety classifier approves routine actions",
  },
};

const DEFAULT_PERMISSION_MODE = PREF_CODING_AGENT_CLAUDE_PERMISSION_MODE.fallback;

export const PERMISSION_MODE_OPTIONS: DropdownOption[] = [
  DEFAULT_PERMISSION_MODE,
  ...CC_PERMISSION_MODES.filter((mode) => mode !== DEFAULT_PERMISSION_MODE),
].map((mode) => ({ value: mode, ...PERMISSION_MODE_COPY[mode] }));

export function isPermissionMode(value: string): value is CcPermissionMode {
  return (CC_PERMISSION_MODES as readonly string[]).includes(value);
}

export function CodingAgentPermissionSection() {
  const mode = currentCodingAgentPermissionMode();

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="coding-agents:permissions">
        Permissions
        <Explainer title="Permissions">
          <p>
            Which of Claude Code's own permission modes its threads run in. Changes apply
            to new sessions.
          </p>
          <p>
            <strong>{PERMISSION_MODE_COPY[DEFAULT_PERMISSION_MODE].label}</strong> is the
            default. Writes inside the session's
            working directories go through, and anything else asks you first. Those are
            the session's own worktree, this workspace's <code>data</code> folder, and{' '}
            <code>/tmp</code>.
          </p>
          <p>
            <strong>Auto</strong> hands each of those to Claude Code's safety classifier
            instead. It reaches things no allowlist can, such as a command that changes
            directory and redirects output in one line.
          </p>
          <p>
            What Auto costs. It ignores a blanket <code>Bash</code> entry in your Claude
            Code permissions, so more commands reach the classifier, each a round-trip. A
            classifier it cannot reach denies the action rather than asking you. And a run
            of denials falls back to asking anyway.
          </p>
        </Explainer>
      </div>
      <div class="list-rows">
        <div class="list-row">
          <div class="list-row-info">
            <div class="title">Claude Code</div>
            <div class="list-row-details list-row-details-prose">
              Codex threads are unaffected: it has no equivalent setting.
            </div>
          </div>
          <div class="list-row-actions">
            <Dropdown
              options={PERMISSION_MODE_OPTIONS}
              value={mode}
              onChange={(v) => {
                if (isPermissionMode(v)) void setCodingAgentPermissionMode(v);
              }}
            />
          </div>
        </div>
      </div>
    </div>
  );
}
