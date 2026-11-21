import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import { showToast } from '../../store/store';
import { getCodingAgentBinaries, setPreference, deletePreference } from '../../api/client';
import type { AgentBinaryStatus } from '../../api/types';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { errorDetail } from '../../utils/errorDetail';
import { LoadableError } from '../shared/LoadableError';
import { Explainer } from '../shared/Explainer';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkBlock, SkText } from '../shared/Skeleton';

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
 * Where this agent's binary resolved from, as a short label. The PATH itself is
 * deliberately NOT in here: it is shown once, in the input below, so the row
 * doesn't print the same long string twice. The two states that have no path to
 * show keep their actionable text (nothing resolved, or the spawn-failure
 * message naming the preference). Pure, unit tested in
 * `codingAgentBinaries.test.ts`.
 */
export function binaryStatusLine(s: AgentBinaryStatus, binary: string): string {
  switch (s.source) {
    case 'override':
      return s.valid ? 'Configured' : (s.error ?? `Configured path ${s.path} is invalid`);
    case 'detected':
      return 'Auto-detected';
    case 'path':
      return 'Found on PATH';
    case 'not-found':
      return `Not found: install ${binary} or set a path below`;
  }
}

/**
 * The binary's own version as a display token (`v2.1.224`), or '' when the
 * engine reported none. Never a placeholder: an unknown version renders as
 * nothing rather than as "unknown".
 */
export function binaryVersionLabel(s: AgentBinaryStatus): string {
  return s.version ? `v${s.version}` : '';
}

/** The override currently stored for this status, or '' when auto-detecting. */
export function storedOverride(s: AgentBinaryStatus): string {
  return s.source === 'override' ? (s.path ?? '') : '';
}

/** One agent's binary. With no `status`, inside a `SkeletonProvider`, it
 *  draws itself as the loading placeholder: the agent's name is known before
 *  the read lands, so only what the read decides shimmers. */
function AgentRow({
  agent,
  status,
  onChanged = () => {},
}: {
  agent: (typeof AGENTS)[number];
  status?: AgentBinaryStatus;
  onChanged?: () => void;
}) {
  const stored = status ? storedOverride(status) : '';
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
  const broken = status?.source === 'override' && !status.valid;
  const missing = status?.source === 'not-found';
  const version = status ? binaryVersionLabel(status) : '';
  return (
    <div class="list-row repo-add-form">
      <div class="list-row-info" style={{ gap: '0.5rem' }}>
        <div class="title">{agent.label}</div>
        {/* Version and source are two FIELDS: `.list-row-details` is a flex
            row whose gap IS the separator, so no glue character between them
            (an explicit one would be double-spaced). */}
        {status ? (
          <div
            class={`list-row-details${broken || missing ? ' error-text' : ''}`}
            data-role="agent-binary-status"
          >
            {version && <span data-role="agent-binary-version">{version}</span>}
            <span>{binaryStatusLine(status, agent.binary)}</span>
          </div>
        ) : <SkText class="list-row-details" as="div" w="9rem" />}
        {/* Input and its actions share one row so Save/Clear sit level with the
            input (not the title). Both action slots are always rendered, disabled
            when their action isn't available, so the input width never shifts as
            you type. */}
        <div class="coding-agent-row-controls">
          <SkBlock w="100%" h="2rem" round>
            <input
              class="device-name-input"
              type="text"
              // The resolved path lives here and nowhere else in the row: as the
              // placeholder while detection owns it, as the value once the user
              // overrides it. The status line above says only where it came from.
              placeholder={status?.path ?? `/path/to/${agent.binary}`}
              value={input}
              onInput={(e) => setInput((e.target as HTMLInputElement).value)}
            />
          </SkBlock>
          <div class="list-row-actions">
            <SkBlock w="3.5rem" h="2rem" round>
              <button
                class="action-btn action-btn-confirm"
                disabled={busy || !dirty || !trimmed}
                onClick={save}
              >
                Save
              </button>
            </SkBlock>
            <SkBlock w="3.75rem" h="2rem" round>
              <button
                class="action-btn action-btn-danger"
                disabled={busy || !stored}
                onClick={clear}
              >
                Clear
              </button>
            </SkBlock>
          </div>
        </div>
      </div>
    </div>
  );
}

/**
 * Settings → Coding Agents → Binaries: where each coding agent's CLI binary
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
  // Pull-to-refresh waits here for the re-read it asked for to land.
  const settles = useRef<Array<() => void>>([]);
  const { loadable: info, showLoading } = useLoadableFetch(getCodingAgentBinaries, [refresh], {
    keepLoadedWhileRefetching: true,
    onSettled: () => {
      const waiting = settles.current;
      settles.current = [];
      for (const settle of waiting) settle();
    },
  });
  const reload = useCallback(() => setRefresh((n) => n + 1), []);
  usePanelRefresh('coding agent binaries', () => new Promise<void>((resolve) => {
    settles.current.push(resolve);
    reload();
  }));

  function body() {
    if (info.status === 'failed') {
      return <LoadableError noun="coding agent binaries" error={info.error} />;
    }
    return (
      <LoadingFade
        showSkeleton={showLoading}
        skeleton={<ListSkeletonOf count={AGENTS.length} containerClass="list-rows" row={(i) => <AgentRow agent={AGENTS[i]!} />} />}
      >
        {info.status === 'loaded' && (
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
        )}
      </LoadingFade>
    );
  }

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="coding-agents:binaries">
        Binaries
        <Explainer title="Binaries">
          <p>
            Which <code>claude</code> / <code>codex</code> binary coding-agent threads run.
          </p>
          <p>
            Auto-detection covers the native installers, Homebrew, and PATH; set an
            explicit path only if detection picks the wrong binary or fails. Changes
            apply to new sessions.
          </p>
        </Explainer>
      </div>
      {body()}
    </div>
  );
}
