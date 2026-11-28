import { cloneElement, type VNode } from 'preact';
import { signal } from '@preact/signals';
import { useEffect, useState } from 'preact/hooks';
import {
  fetchEngineBuilds,
  switchRunningToNewestBuild,
  type EngineBuilds,
  type SwitchOutcome,
  type WorkspaceBuild,
} from '../../api/client/control';
import { setLoadingIfFresh, toFailed, type Loadable } from '../../store/types';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { LoadingFade } from '../shared/LoadingFade';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeletonOf, SkText } from '../shared/Skeleton';
import { Explainer } from '../shared/Explainer';
import { showConfirm, showToast } from '../../store/store';
import { errorDetail } from '../../utils/errorDetail';

const engineBuilds = signal<Loadable<EngineBuilds>>({ status: 'not-loaded' });

async function loadEngineBuilds(): Promise<void> {
  setLoadingIfFresh(engineBuilds);
  try {
    engineBuilds.value = { status: 'loaded', data: await fetchEngineBuilds() };
  } catch (e) {
    engineBuilds.value = toFailed(e);
  }
}

const SHORT_BUILD = 12;

function buildLabel(id: string | null): string {
  if (!id) return 'unknown build';
  return id.length > SHORT_BUILD ? id.slice(0, SHORT_BUILD) : id;
}

export function workspaceBuildState(row: WorkspaceBuild, newest: string | null): string {
  if (!row.running) return 'stopped, starts on the newest build';
  if (row.unanswered) return 'running, not answering';
  if (row.behind) return 'on an older build';
  if (newest !== null && row.build_id === newest) return 'on the newest build';
  return 'nothing newer to switch to';
}

export function workspaceBuildRow(row?: WorkspaceBuild, newest: string | null = null): VNode {
  return (
    <div class="list-row">
      <div class="list-row-info">
        <SkText class="list-row-name" w="45%">
          <span class="title">{row?.name}</span>
        </SkText>
        <SkText class="list-row-details" as="div" w="55%">
          <span class="list-row-type">{row && workspaceBuildState(row, newest)}</span>
          {row?.running && !row.unanswered && (
            <span data-tooltip={row.build_id ?? undefined}>
              <code>{buildLabel(row.build_id)}</code>
            </span>
          )}
        </SkText>
      </div>
    </div>
  );
}

export function switchOutcomeMessage(outcome: SwitchOutcome): { text: string; failed: boolean } {
  const parts: string[] = [];
  if (outcome.switched.length > 0) parts.push(`${outcome.switched.length} switching to the newest build`);
  if (outcome.skipped.length > 0) {
    parts.push(`${outcome.skipped.length} left alone (stopped, already switched or not answering)`);
  }
  if (outcome.failed.length > 0) {
    parts.push(`couldn't switch ${outcome.failed.map((f) => `"${f.id}" (${f.error})`).join(', ')}`);
  }
  return { text: parts.join('; ') || 'Nothing to switch', failed: outcome.failed.length > 0 };
}

async function switchAllRunning(behind: number): Promise<void> {
  const ok = await showConfirm(
    `Restart ${behind} running workspace${behind === 1 ? '' : 's'} on the newest build?\n\n` +
      'Each restarts as its own Switch to new version would. Stopped workspaces stay stopped.',
    'Switch',
    { variant: 'default' },
  );
  if (!ok) return;
  try {
    const { text, failed } = switchOutcomeMessage(await switchRunningToNewestBuild());
    showToast(text, failed ? 'error' : 'success');
  } catch (e) {
    showToast(`Couldn't switch the running workspaces: ${errorDetail(e)}`, 'error');
  }
  await loadEngineBuilds();
}

export function EngineBuildsSection() {
  const loadable = engineBuilds.value;
  const showSkeleton = useDelayedLoading(loadable);
  const [switching, setSwitching] = useState(false);

  useEffect(() => {
    void loadEngineBuilds();
  }, []);
  usePanelRefresh('the workspace builds', loadEngineBuilds);

  const builds = loadable.status === 'loaded' ? loadable.data : null;
  const behind = builds?.workspaces.filter((w) => w.running && w.behind).length ?? 0;

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="updates:workspace-builds">
        Workspace builds
        <Explainer title="Workspace builds">
          <p>
            Every workspace on this gateway runs the same engine. A stopped or
            new workspace starts on the newest build.
          </p>
          <p>
            A running workspace keeps its build until it switches, through its
            own Switch to new version or Switch all running here. Nothing
            restarts an idle workspace on its own.
          </p>
          <p>
            Whether a new build waits for coding threads is a setting of each
            workspace.
          </p>
        </Explainer>
      </div>

      {loadable.status === 'failed' && (
        <LoadableError noun="the workspace builds" error={loadable.error} />
      )}

      <LoadingFade
        showSkeleton={showSkeleton}
        skeleton={
          <ListSkeletonOf containerClass="list-rows" count={2} row={() => workspaceBuildRow()} />
        }
      >
        {builds &&
          (builds.workspaces.length > 0 ? (
            <div class="list-rows">
              {builds.workspaces.map((row) =>
                cloneElement(workspaceBuildRow(row, builds.newest), { key: row.id }),
              )}
            </div>
          ) : (
            <div class="empty-state">No workspaces on this gateway.</div>
          ))}
      </LoadingFade>

      {behind > 0 && (
        <div class="system-actions">
          <button
            class="action-btn"
            disabled={switching}
            onClick={() => {
              setSwitching(true);
              void switchAllRunning(behind).finally(() => setSwitching(false));
            }}
          >
            Switch all running
          </button>
        </div>
      )}
    </div>
  );
}
