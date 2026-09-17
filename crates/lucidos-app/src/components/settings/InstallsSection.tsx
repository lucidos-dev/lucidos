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
import { toFailed, type Loadable } from '../../store/types';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { LoadingFade } from '../shared/LoadingFade';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeletonOf, SkText } from '../shared/Skeleton';
import { Explainer } from '../shared/Explainer';
import { copyToClipboard } from '../../utils/clipboard';

/** What the machine carries. One read, when this page is opened.
 *
 *  No version counter and no poll: installs change when somebody runs an
 *  installer in a terminal, which emits no event this app could subscribe to.
 *  The gateway rescans on every request, so reopening the page is the refresh.
 */
const installInventory = signal<Loadable<InstallInventory>>({ status: 'not-loaded' });

async function loadInstalls(): Promise<void> {
  installInventory.value = { status: 'loading' };
  try {
    installInventory.value = { status: 'loaded', data: await fetchInstallInventory() };
  } catch (e) {
    installInventory.value = toFailed(e);
  }
}

/** How each vehicle reads to a person. Mirrors `InstallKind::label` in
 *  `crates/lucidos-installs/src/lib.rs`; the wire carries the kind, never the
 *  label, so the closed union type is what keeps the two in step. */
const KIND_LABEL: Record<InstallKind, string> = {
  'desktop-app': 'Desktop app',
  'headless-installer': 'Installer',
  'source-checkout': 'Source checkout',
};

/** One install, as a row.
 *
 *  A plain builder rather than a component, so its output is testable without a
 *  render. Skeleton mode is the `sk` flag plus the `SkText` leaves, which read
 *  the provider `ListSkeletonOf` wraps them in. Same markup either way, so the
 *  placeholder mirrors the row by construction.
 */
export function installRow(install?: InstallRecord, sk = false): VNode {
  const path = install?.root ?? install?.data_dir ?? null;
  return (
    <div class="list-row">
      <div class="list-row-info">
        <SkText class="list-row-name" w="55%">
          <span class="title">{install?.name}</span>
          {install?.running_here && <span class="install-live"> serving this workspace</span>}
        </SkText>
        <SkText class="list-row-details" as="div" w="40%">
          {/* A kind this build has no label for falls back to the WIRE value,
              never to a blank. A newer gateway can name a vehicle this client
              predates, and an empty cell would read as "no kind". */}
          <span class="list-row-type">
            {install && (KIND_LABEL[install.kind] ?? install.kind)}
          </span>
          {/* An unreadable version says so. A blank would read as zero. */}
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
      </div>
    </div>
  );
}

/** The warning for one contended port.
 *
 *  Only a contended port earns one. Two installs on two ports are a supported
 *  setup, and warning about that would be a nag on every developer machine.
 */
export function installConflictNotice(conflict: InstallPortConflict): VNode {
  return (
    <div class="system-notice">
      <strong>
        {conflict.installs.length} installs are set up to use port {conflict.port}.
      </strong>{' '}
      Only one can answer, and whichever started first wins. So this workspace may be
      served by an install you did not choose. Remove the one you do not want, with
      the command listed below.
    </div>
  );
}

/** Every Lucidos install on this machine, and a warning when two want one port. */
export function InstallsSection() {
  const loadable = installInventory.value;
  const showSkeleton = useDelayedLoading(loadable);

  useEffect(() => {
    void loadInstalls();
  }, []);

  const inventory = loadable.status === 'loaded' ? loadable.data : null;
  // A removal command is offered only where there is a choice to make. With one
  // install there is nothing to disambiguate, and the command would be an
  // invitation nobody asked for.
  const several = (inventory?.installs.length ?? 0) > 1;

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="system:installs">
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

      {/* `cloneElement` for the key rather than a wrapper div: these are flex
          children of their container, and an unstyled box between the two
          would take the child's place in that layout. Same idiom
          `ListSkeletonOf` uses to key its fanned-out rows. */}
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
                cloneElement(installRow(install), { key: install.name }),
              )}
            </div>
          ) : (
            // Loaded and empty is not the same as still loading, and it is not
            // nothing: this page is being served BY an install, so finding
            // none means the scan could not read the machine.
            <div class="empty-state">
              No installs found. Something is serving this page, so the scan could
              not read this machine.
            </div>
          ))}
      </LoadingFade>

      {several &&
        inventory?.installs.map((install) => (
          <div key={`removal-${install.name}`} class="system-footnote">
            Remove {install.name}: <code>{install.removal}</code>{' '}
            <button class="accent-link" onClick={() => copyToClipboard(install.removal)}>
              Copy
            </button>
          </div>
        ))}
    </div>
  );
}
