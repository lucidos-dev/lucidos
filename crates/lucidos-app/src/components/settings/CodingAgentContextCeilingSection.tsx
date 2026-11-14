import { currentContextCeiling, setContextCeiling } from '../../store/actions/preferences';
import { Dropdown, type DropdownOption } from '../shared/Dropdown';
import { Explainer } from '../shared/Explainer';
import { PREFERENCE_CATALOG } from '@lucidos/preference-catalog';

const DEFAULT_CEILING = PREFERENCE_CATALOG.coding_agent_context_ceiling.fallback;

/** The values the picker offers. Any other the engine accepts, set through
 *  the CLI or a thread, still shows as the current one. */
const OFFERED = ['off', '200k', '300k', '400k', '500k', '30%', '50%'];

function ceilingLabel(value: string): string {
  if (value === 'off') return 'Off';
  return value.endsWith('%') ? `${value} of the window` : value;
}

/** The picker's options, with the catalog default marked, and `current` kept
 *  when it is not one the picker offers. Pure, unit tested. */
export function contextCeilingOptions(current: string): DropdownOption[] {
  const values = OFFERED.includes(current) ? OFFERED : [...OFFERED, current];
  return values.map((value) => ({
    value,
    label: ceilingLabel(value),
    ...(value === DEFAULT_CEILING && { description: 'Default' }),
    ...(value === 'off' && { description: 'Sessions grow until the backend compacts' }),
  }));
}

/**
 * Settings → Coding Agents → Context ceiling: the workspace's *context
 * ceiling* (ADR 0629), the size at which a coding-agent session hands off to a
 * fresh session. A thread or trigger may set its own, and a child takes its
 * parent's.
 */
export function CodingAgentContextCeilingSection() {
  const ceiling = currentContextCeiling();

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="coding-agents:context-ceiling">
        Context ceiling
        <Explainer title="Context ceiling">
          <p>
            Every reply rereads a session's whole context, so a large session costs more on
            every turn. Past this size, the session hands off to a fresh session in the same
            thread at its next turn end.
          </p>
          <p>
            The agent writes a handoff narrative first, while its cache is warm. Your next
            message then starts the fresh session. You can return to the old session from the
            card above the composer.
          </p>
          <p>
            A ceiling never goes above three quarters of the model's context window, nor below
            100k. A thread or a trigger can set its own.
          </p>
        </Explainer>
      </div>
      <div class="list-rows">
        <div class="list-row">
          <div class="list-row-info">
            <div class="title">Hand off past</div>
            <div class="list-row-details list-row-details-prose">
              Claude Code and Codex threads that set no ceiling of their own.
            </div>
          </div>
          <div class="list-row-actions">
            <Dropdown
              options={contextCeilingOptions(ceiling)}
              value={ceiling}
              onChange={(v) => void setContextCeiling(v)}
            />
          </div>
        </div>
      </div>
    </div>
  );
}
