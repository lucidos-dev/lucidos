import { useEffect, useRef, useState } from 'preact/hooks';
import { API, json, mutatingFetch, throwIfNotOk } from '../../api/client/_core';
import { currentAgentsUnsandboxed } from '../../store/actions/preferences';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { loadingIfFresh, toFailed, type Loadable } from '../../store/types';
import { LoadingFade } from '../shared/LoadingFade';
import { LoadableError } from '../shared/LoadableError';
import { SkeletonProvider, SkBlock } from '../shared/Skeleton';
import { errorDetail } from '../../utils/errorDetail';
import { Explainer } from '../shared/Explainer';
import './private-paths.css';

type Scope = 'machine' | 'workspace';
type Enforcement = 'full' | 'read-only' | 'not-enforced' | 'unavailable';

interface PrivatePaths {
  machine: string[];
  workspace: string[];
  enforcement: Enforcement;
}

const enforcementLabels: Record<Enforcement, string> = {
  full: 'Full protection',
  'read-only': 'Read protection only',
  'not-enforced': 'Not enforced',
  unavailable: 'Sandbox unavailable',
};

function PathList({ title, paths, disabled, save }: {
  title: string;
  paths: string[];
  disabled: boolean;
  save: (paths: string[]) => Promise<boolean>;
}) {
  const [entry, setEntry] = useState('');
  const [adding, setAdding] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => { if (adding) input.current?.focus(); }, [adding]);

  async function add(event: Event) {
    event.preventDefault();
    if (!entry || disabled) return;
    if (await save([...paths, entry])) {
      setEntry('');
      setAdding(false);
    }
  }

  return (
    <>
      <div class="settings-row">
        <span class="settings-row-label">{title}</span>
      </div>
      <div class="allowlist-rows">
        {paths.length === 0 && <div class="allowlist-empty">No paths added.</div>}
        {paths.map((path) => (
          <div class="allowlist-row" key={path}>
            <code class="allowlist-row-input private-paths-value">
              {path.split('/').map((part, index) => (
                index === 0 ? part : <span key={index}>/<wbr />{part}</span>
              ))}
            </code>
            <button
              type="button"
              class="settings-option private-paths-action"
              aria-label={`Remove ${path}`}
              disabled={disabled}
              onClick={() => void save(paths.filter((item) => item !== path))}
            >
              Remove
            </button>
          </div>
        ))}
      </div>
      {adding ? (
        <form class="allowlist-row" onSubmit={add}>
          <input
            class="allowlist-row-input"
            aria-label={`New ${title.toLowerCase()} private path`}
            placeholder="/absolute/path"
            value={entry}
            spellcheck={false}
            required
            ref={input}
            onInput={(event) => setEntry(event.currentTarget.value)}
            onKeyDown={(event) => { if (event.key === 'Escape') setAdding(false); }}
          />
          <button type="submit" class="settings-option private-paths-action" disabled={disabled || !entry}>
            Add path
          </button>
        </form>
      ) : (
        <div class="allowlist-actions">
          <span class="allowlist-actions-spacer" />
          <button
            type="button"
            class="settings-option private-paths-action"
            disabled={disabled}
            onClick={() => setAdding(true)}
          >
            Add path
          </button>
        </div>
      )}
    </>
  );
}

function SandboxProtectionRow({ status }: { status?: Enforcement }) {
  return (
    <div class="settings-row private-paths-status">
      <span class="settings-row-label">Sandbox protection</span>
      <SkBlock w="8rem" h="1rem" round>
        <span>{status && enforcementLabels[status]}</span>
      </SkBlock>
    </div>
  );
}

export function PrivatePathsSection() {
  const [loadable, setLoadable] = useState<Loadable<PrivatePaths>>({ status: 'not-loaded' });
  const policy = loadable.status === 'loaded' ? loadable.data : undefined;
  const showLoading = useDelayedLoading(loadable);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const version = useRef(0);
  const unsandboxed = currentAgentsUnsandboxed();

  async function reload() {
    const started = ++version.current;
    setLoadable(loadingIfFresh);
    try {
      const value = await json<PrivatePaths>(`${API}/private-paths`);
      if (started === version.current) setLoadable({ status: 'loaded', data: value });
    } catch (err) {
      if (started === version.current) setLoadable(toFailed(err));
    }
  }

  useEffect(() => {
    void reload();
    return () => { version.current++; };
  }, [unsandboxed]);
  usePanelRefresh('private paths', saving ? null : reload);

  async function save(scope: Scope, paths: string[]) {
    if (!policy || saving) return false;
    version.current++;
    setSaving(true);
    setError('');
    try {
      const response = await mutatingFetch(`${API}/private-paths`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ scope, paths, previous: policy[scope] }),
      });
      if (response.status === 409) void reload();
      await throwIfNotOk(response);
      setLoadable({ status: 'loaded', data: await response.json() as PrivatePaths });
      return true;
    } catch (err) {
      setError(errorDetail(err));
      return false;
    } finally {
      setSaving(false);
    }
  }

  const saveMachine = (paths: string[]) => { return save('machine', paths); };
  const saveWorkspace = (paths: string[]) => { return save('workspace', paths); };

  return (
    <section class="settings-section private-paths-section">
      <div class="settings-section-title" data-search-anchor="private-paths">
        Private paths
        <Explainer title="Private paths">
          <p>Add an absolute file or folder path outside workspaces and registered repositories. Machine-wide paths apply across workspaces and take precedence over repository access.</p>
          <p>Older Linux sandboxes protect reads and can still allow a protected file to be truncated. When sandbox enforcement is off, agent processes can access private paths.</p>
          <p>Linux can limit creation in parent folders, so private paths under system temporary folders are refused.</p>
        </Explainer>
      </div>
      <LoadingFade showSkeleton={showLoading} skeleton={<SkeletonProvider><SandboxProtectionRow /></SkeletonProvider>}>
        {policy && (
          <>
            <SandboxProtectionRow status={unsandboxed ? 'not-enforced' : policy.enforcement} />
            <p class="settings-section-desc">Running agent sessions pick up changes on their next start.</p>
            <PathList title="Machine-wide" paths={policy.machine} disabled={saving} save={saveMachine} />
            <PathList title="This workspace" paths={policy.workspace} disabled={saving} save={saveWorkspace} />
          </>
        )}
        {loadable.status === 'failed' && <LoadableError noun="private paths" error={loadable.error} onRetry={() => void reload()} />}
      </LoadingFade>
      {error && <p role="alert" class="settings-section-desc">{error}</p>}
    </section>
  );
}
