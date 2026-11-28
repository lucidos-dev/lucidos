import { cloneElement, type VNode } from 'preact';
import { signal } from '@preact/signals';
import { useEffect } from 'preact/hooks';
import {
  fetchInstallInventory,
  type InstallInventory,
  type InstallKind,
  type InstallPortConflict,
  type InstallRecord,
} from '../../api/client/control';
import { setLoadingIfFresh, toFailed, type Loadable } from '../../store/types';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { LoadingFade } from '../shared/LoadingFade';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeletonOf, SkText } from '../shared/Skeleton';
import { Explainer } from '../shared/Explainer';
import { copyToClipboard } from '../../utils/clipboard';

const installInventory = signal<Loadable<InstallInventory>>({ status: 'not-loaded' });

async function loadInstalls(): Promise<void> {
  setLoadingIfFresh(installInventory);
  try {
    installInventory.value = { status: 'loaded', data: await fetchInstallInventory() };
  } catch (e) {
    installInventory.value = toFailed(e);
  }
}

const KIND_LABEL: Record<InstallKind, string> = {
  'desktop-app': 'Desktop app',
  'headless-installer': 'Installer',
  'source-checkout': 'Source checkout',
};

export function installRow(install?: InstallRecord, sk = false, contended = false): VNode {
  const path = install?.root ?? install?.data_dir ?? null;
  return (
    <div class="list-row">
      <div class="list-row-info">
        <SkText class="list-row-name" w="55%">
          <span class="title">{install?.name}</span>
          {install?.running_here && <span class="install-live"> serving this workspace</span>}
        </SkText>
        <SkText class="list-row-details" as="div" w="40%">
          <span class="list-row-type">
            {install && (KIND_LABEL[install.kind] ?? install.kind)}
          </span>
          <span>version {install?.version ?? 'unknown'}</span>
          <span>port {install?.port ?? 'unknown'}</span>
        </SkText>
        {(sk || path) && (
          <SkText class="list-row-details list-row-details-prose" as="div" w="70%">
            {path}
          </SkText>
        )}
        {!sk &&
          install?.agents.map((agent) => (
            <div key={agent.path} class="list-row-details list-row-details-prose">
              Starts on its own: <code>{agent.label}</code>
            </div>
          ))}
        {install && contended && removalLine(install)}
      </div>
    </div>
  );
}

function removalLine(install: InstallRecord): VNode {
  return (
    <div class="list-row-details list-row-details-prose">
      To remove:{' '}
      {install.kind === 'headless-installer' ? (
        <>
          <code>{install.removal}</code>{' '}
          <button class="accent-link" onClick={() => copyToClipboard(install.removal)}>
            Copy
          </button>
        </>
      ) : (
        install.removal
      )}
    </div>
  );
}

export function installConflictNotice(conflict: InstallPortConflict): VNode {
  return (
    <div class="system-notice">
      <strong>
        {conflict.installs.length} installs are set up to use port {conflict.port}.
      </strong>{' '}
      Only one can answer, and whichever started first wins. So this workspace may be
      served by an install you did not choose. Remove the one you do not want: its
      row below says how.
    </div>
  );
}

export function InstallsSection() {
  const loadable = installInventory.value;
  const showSkeleton = useDelayedLoading(loadable);

  useEffect(() => {
    void loadInstalls();
  }, []);
  usePanelRefresh('the installs on this machine', loadInstalls);

  const inventory = loadable.status === 'loaded' ? loadable.data : null;
  const contended = new Set(inventory?.conflicts.flatMap((c) => c.installs));

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="updates:installs">
        Installs
        <Explainer title="Installs">
          <p>
            Every copy of Lucidos this machine can start, however it was
            installed: the app, a <code>curl … | sh</code> install, or a source
            checkout.
          </p>
          <p>
            They coexist happily, each on its own port. That is a supported setup
            and nothing here complains about it.
          </p>
          <p>
            Two of them wanting the SAME port is the problem. Only one can answer,
            whichever started first, so the app can end up talking to an engine
            from an install you forgot about. That is what is warned about here.
          </p>
        </Explainer>
      </div>

      {loadable.status === 'failed' && (
        <LoadableError noun="the installs on this machine" error={loadable.error} />
      )}

      {inventory?.conflicts.map((conflict) =>
        cloneElement(installConflictNotice(conflict), { key: conflict.port }),
      )}

      <LoadingFade
        showSkeleton={showSkeleton}
        skeleton={
          <ListSkeletonOf
            containerClass="list-rows"
            count={2}
            row={() => installRow(undefined, true)}
          />
        }
      >
        {inventory &&
          (inventory.installs.length > 0 ? (
            <div class="list-rows">
              {inventory.installs.map((install) =>
                cloneElement(installRow(install, false, contended.has(install.name)), {
                  key: install.name,
                }),
              )}
            </div>
          ) : (
            <div class="empty-state">
              No installs found. Something is serving this page, so the scan could
              not read this machine.
            </div>
          ))}
      </LoadingFade>
    </div>
  );
}
