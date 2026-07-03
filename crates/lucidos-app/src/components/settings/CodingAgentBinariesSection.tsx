import { useCallback, useEffect, useState } from 'preact/hooks';
import { showToast } from '../../store/store';
import { getCodingAgentBinaries, setPreference, deletePreference } from '../../api/client';
import type { AgentBinaryStatus } from '../../api/types';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { errorDetail } from '../../utils/errorDetail';
import { LoadableError } from '../shared/LoadableError';

const AGENTS = [
  {
    key: 'claude_code',
    label: 'Claude Code',
    binary: 'claude',
    pref: 'coding_agent_claude_path',
  },
  {
    key: 'codex',
    label: 'Codex',
    binary: 'codex',
    pref: 'coding_agent_codex_path',
  },
] as const;

type AgentKey = (typeof AGENTS)[number]['key'];

/**
 * Human status line for an agent's current binary resolution. Pure — unit
 * tested in `codingAgentBinaries.test.ts`.
 */
export function binaryStatusLine(s: AgentBinaryStatus, binary: string): string {
  switch (s.source) {
    case 'override':
      return s.valid
        ? `Using configured path ${s.path}`
        : (s.error ?? `Configured path ${s.path} is invalid`);
    case 'detected':
      return `Auto-detected at ${s.path}`;
    case 'path':
      return `Found on PATH at ${s.path}`;
    case 'not-found':
      return `Not found — install ${binary} or set a path below`;
  }
}

/** The override currently stored for this status, or '' when auto-detecting. */
export function storedOverride(s: AgentBinaryStatus): string {
  return s.source === 'override' ? (s.path ?? '') : '';
}

function AgentRow({
  agent,
  status,
  onChanged,
}: {
  agent: (typeof AGENTS)[number];
  status: AgentBinaryStatus;
  onChanged: () => void;
}) {
  const stored = storedOverride(status);
  const [input, setInput] = useState(stored);
  const [busy, setBusy] = useState(false);
  // Re-seed the input when a reload changes the stored override (e.g. after
  // Clear); unrelated reloads keep local edits (`stored` unchanged → no run).
  useEffect(() => {
    setInput(stored);
  }, [stored]);

  const save = useCallback(async () => {
    const value = input.trim();
    if (!value) return;
    setBusy(true);
    try {
      const result = await setPreference(agent.pref, value);
      if (!result.success) {
        showToast(result.error || `Failed to save ${agent.label} path`, 'error');
        return;
      }
      showToast(`${agent.label} path saved — applies to new sessions`, 'success');
      onChanged();
    } catch (e) {
      showToast(`Failed to save ${agent.label} path: ${errorDetail(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }, [agent, input, onChanged]);

  const clear = useCallback(async () => {
    setBusy(true);
    try {
      const result = await deletePreference(agent.pref);
      if (!result.success) {
        showToast(result.error || `Failed to clear ${agent.label} path`, 'error');
        return;
      }
      showToast(`${agent.label} path cleared — auto-detecting again`, 'success');
      onChanged();
    } catch (e) {
      showToast(`Failed to clear ${agent.label} path: ${errorDetail(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }, [agent, onChanged]);

  const trimmed = input.trim();
  const dirty = trimmed !== stored;
  const broken = status.source === 'override' && !status.valid;
  const missing = status.source === 'not-found';
  return (
    <div class="list-row repo-add-form">
      <div class="list-row-info" style={{ gap: '0.5rem' }}>
        <div class="title">{agent.label}</div>
        <div
          class={`list-row-details${broken || missing ? ' error' : ''}`}
          data-role="agent-binary-status"
        >
          {binaryStatusLine(status, agent.binary)}
        </div>
        {/* Input and its actions share one row so Save/Clear sit level with the
            input (not the title). Both action slots are always rendered, disabled
            when their action isn't available, so the input width never shifts as
            you type. */}
        <div class="coding-agent-row-controls">
          <input
            class="device-name-input"
            type="text"
            placeholder={
              status.source === 'detected' || status.source === 'path'
                ? `${status.path} (auto-detected)`
                : `/path/to/${agent.binary}`
            }
            value={input}
            onInput={(e) => setInput((e.target as HTMLInputElement).value)}
          />
          <div class="list-row-actions">
            <button
              class="action-btn action-btn-confirm"
              disabled={busy || !dirty || !trimmed}
              onClick={save}
            >
              Save
            </button>
            <button
              class="action-btn action-btn-danger"
              disabled={busy || !stored}
              onClick={clear}
            >
              Clear
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

/**
 * Settings → System → Coding agents: where each coding agent's CLI binary
 * resolves from, with an optional per-agent path override.
 *
 * Detection is LIVE (recomputed by the engine per request) — only an explicit
 * override is persisted (`coding_agent_*_path` preference), so a Homebrew
 * upgrade or installer move self-heals instead of leaving a stale stored
 * path. A saved path that stops resolving fails agent spawns with an error
 * naming the setting, and this section shows the same error.
 */
export function CodingAgentBinariesSection() {
  // Bumped after a save/clear so the shared hook refetches (with stale-fetch
  // cancellation — a slow first response can't clobber the fresher reload).
  const [refresh, setRefresh] = useState(0);
  const { loadable: info, showLoading } = useLoadableFetch(getCodingAgentBinaries, [refresh]);
  const reload = useCallback(() => setRefresh((n) => n + 1), []);

  function body() {
    if (info.status === 'failed') {
      return <LoadableError noun="coding agent binaries" error={info.error} />;
    }
    if (info.status !== 'loaded') {
      if (!showLoading) return null;
      return <div class="empty-state">Loading…</div>;
    }
    return (
      <div class="list-rows">
        {AGENTS.map((agent) => (
          <AgentRow
            key={agent.key}
            agent={agent}
            status={info.data[agent.key as AgentKey]}
            onChanged={reload}
          />
        ))}
      </div>
    );
  }

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="system:coding-agents">
        Coding agents
      </div>
      <p class="settings-section-desc">
        Which <code>claude</code> / <code>codex</code> binary coding-agent threads run. Auto-detection
        covers the native installers, Homebrew, and PATH; set an explicit path only if detection
        picks the wrong binary or fails. Changes apply to new sessions.
      </p>
      {body()}
    </div>
  );
}
