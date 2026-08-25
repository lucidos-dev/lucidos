import {
  CC_PERMISSION_MODES,
  currentCodingAgentPermissionMode,
  setCodingAgentPermissionMode,
  type CcPermissionMode,
} from '../../store/actions/preferences';
import { Dropdown, type DropdownOption } from '../shared/Dropdown';
import { Explainer } from '../shared/Explainer';

/** The picker's options, in the order a user should weigh them: the safe
 *  default first. Each carries its one-line trade-off as a second row, so the
 *  cost is on screen while choosing, not only in the explainer. */
export const PERMISSION_MODE_OPTIONS: DropdownOption[] = [
  {
    value: 'accept-edits',
    label: 'Accept edits',
    description: 'You approve anything outside the working directories',
  },
  {
    value: 'auto',
    label: 'Auto',
    description: "Claude Code's safety classifier approves routine actions",
  },
];

/** Guards against a value the engine would reject, so a stale or hand-edited
 *  preference cannot be re-saved from here. Pure, unit tested. */
export function isPermissionMode(value: string): value is CcPermissionMode {
  return (CC_PERMISSION_MODES as readonly string[]).includes(value);
}

/**
 * Settings → Coding Agents → Permissions: which of Claude Code's own permission
 * modes its threads run in.
 *
 * It has to be a Lucidos setting. The engine passes `--permission-mode` on
 * every spawn, and Claude Code prefers a CLI value to any settings file. So a
 * user cannot choose the mode from their own `~/.claude/settings.json`. Codex
 * has no equivalent and ignores the key.
 *
 * Reads through `currentCodingAgentPermissionMode` and writes through
 * `setCodingAgentPermissionMode`, both signal-backed. The write applies
 * locally before the request, so the picker never waits on a round trip.
 * Delivery is parked and retried rather than toasted when a suspended PWA
 * aborts the fetch. A raw `setPreference` here would reintroduce that loss.
 */
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
            <strong>Accept edits</strong> is the default. Writes inside the session's
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
