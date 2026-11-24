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

export function binaryVersionLabel(s: AgentBinaryStatus): string {
  return s.version ? `v${s.version}` : '';
}

export function storedOverride(s: AgentBinaryStatus): string {
  return s.source === 'override' ? (s.path ?? '') : '';
}

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
        {status ? (
          <div
            class={`list-row-details${broken || missing ? ' error-text' : ''}`}
            data-role="agent-binary-status"
          >
            {version && <span data-role="agent-binary-version">{version}</span>}
            <span>{binaryStatusLine(status, agent.binary)}</span>
          </div>
        ) : <SkText class="list-row-details" as="div" w="9rem" />}
        <div class="coding-agent-row-controls">
          <SkBlock w="100%" h="2rem" round>
            <input
              class="device-name-input"
              type="text"
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

export function CodingAgentBinariesSection() {
  const [refresh, setRefresh] = useState(0);
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
