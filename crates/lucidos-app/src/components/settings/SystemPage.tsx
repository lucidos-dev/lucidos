import { useCallback, useEffect } from 'preact/hooks';
import {
  appUpdateCheckError,
  appUpdateCheckInFlight,
  appUpdateProgress,
  connectionStatus,
  enginePackaged,
  engineStartedAt,
  engineVersion,
  latestEngineVersion,
  latestTauriAppNotes,
  lucidosRelease,
  releaseCheck,
  lucidosReleaseDirty,
  restartRequired,
  serviceWorkerBuildId,
  showToast,
  updateAvailable,
  visibleWorkspaceName,
  workspacePath,
} from '../../store/store';
import { confirmAndRestartEngine } from '../../store/actions/chat-changes';
import {
  canCheckForUpdatesHere,
  canInstallUpdateHere,
  followUpdateRoute,
  sessionCanInstall,
  updateControlLabel,
  type UpdateRoute,
} from '../../store/actions/app-update';
import { updateGuidance } from './updateGuidance';
import { packagedUpdateVersion } from '../../store/packagedUpdate';
import { setReleaseCheckConfig } from '../../api/client/control';
import { appUpdateNarration } from '../../store/progressDialogCopy';
import { cancelAppUpdate } from '../../utils/tauri';
import { openWhatsNew } from '../../store/actions/menu';
import { requestServiceWorkerBuildId, refreshClient } from '../../hooks/sw-update';
import { formatBuildId } from '../../utils/buildId';
import { clientVersionLabel } from '../../utils/clientVersion';
import { isNewerVersion } from '../../utils/version';
import { copyToClipboard } from '../../utils/clipboard';
import { formatShortTime } from '../../utils/formatTime';
import { connectionNotice } from '../../utils/connectionNotice';
import { errorDetail } from '../../utils/errorDetail';
import { BackupSection } from './BackupSection';
import { DiskUsagePage } from './DiskUsagePage';
import { MemoryInspector } from './MemoryInspector';
import { EnvironmentVariablesPage } from './EnvironmentVariablesPage';
import { DebuggingSection } from './DebuggingSection';
import { CommunicationSurfacesPage } from './CommunicationSurfacesPage';
import { WhatsNewPage } from './WhatsNewPage';
import { ReleaseNoticesPage } from './ReleaseNoticesPage';
import { restartControlHome } from './restartControl';
import { Explainer } from '../shared/Explainer';
import { ThreadQueueView } from '../thread-queue/ThreadQueueView';

/** The SPA origin, read lazily so importing this module never touches the DOM. */
function getApiUrl(): string {
  return typeof window !== 'undefined' && window.location ? window.location.origin : '';
}

/** Which sub-page this renders. `SystemSubmenu` is the list that reaches them,
 *  and `SettingsView.renderSubview` maps each subview key onto one of these. */
export type SystemPanel = 'overview' | 'release-notices' | 'whats-new' | 'thread-queue' | 'backup' | 'memory' | 'disk-usage' | 'environment-variables' | 'debugging' | 'communication-surfaces';

export function SystemPage({ panel }: { panel: SystemPanel }) {
  const status = connectionStatus.value;
  const name = visibleWorkspaceName.value;
  // Non-null for exactly the states that are not `connected`, which is what the
  // Connection block below renders its state word AND its explanation from. The
  // FULL detail, like the bar: this page is where a puzzled user comes to read
  // the whole state, and it has a paragraph's room to answer in.
  const notice = connectionNotice(status, name, 'full');
  const path = workspacePath.value;
  const startedAt = engineStartedAt.value;
  const release = lucidosRelease.value;
  const releaseDirty = lucidosReleaseDirty.value;
  const engineVer = engineVersion.value;
  const latestEngineVer = latestEngineVersion.value;
  const restart = restartRequired.value;
  const update = updateAvailable.value;
  // Overview owns the restart control only in dev, where it genuinely rebuilds.
  // On a packaged install it lives in System > Debugging instead, labelled
  // "Restart Engine" (see restartControl.ts). Exactly one of the two renders.
  //
  // Dropping the button here does NOT strand the `restart` notice below, which
  // says "restart to activate": `restart` cannot be true while packaged. Both
  // routes that set it are dead there. The outdated-release check compares
  // against `latest_engine_version`, which the engine derives from the repo root
  // and so reports as the string `'unknown'` on a packaged install, and
  // `isNewerVersion` coerces that to NaN and returns false. And an applied
  // restart-requiring change needs a Lucidos-source coding-agent thread, which
  // `packaged` (engine-side `!has_lucidos_source()`, resolved from the running
  // binary's own path) is precisely the absence of. If packaged ever starts
  // reporting a real engine version, this notice needs to point at Debugging.
  const ownsRestart = restartControlHome(enginePackaged.value) === 'overview';
  const swBuildId = serviceWorkerBuildId.value;
  const swBuildLabel = swBuildId ? formatBuildId(swBuildId) : null;

  useEffect(() => {
    requestServiceWorkerBuildId();
  }, []);

  const handleRefresh = useCallback(() => {
    refreshClient();
  }, []);

  /** Turn the machine-global release check off or back on. Writes
   *  `~/.lucidos/updates.toml` through the gateway, which re-reads it on every
   *  tick, so the change binds without a restart. */
  const setReleaseCheckEnabled = useCallback(async (enabled: boolean) => {
    try {
      releaseCheck.value = await setReleaseCheckConfig({ enabled });
    } catch (e) {
      showToast(`Couldn't save the update-check setting: ${errorDetail(e)}`, 'error');
    }
  }, []);


  const hasEngineUpdate = engineVer && latestEngineVer && isNewerVersion(latestEngineVer, engineVer);
  const tauriClientVersion = window.__LUCIDOS_APP_VERSION__;
  // Shared with the Lucidos menu's identity row, which names the same thing.
  const clientVersion = clientVersionLabel();
  // This page is where `updateRoute`'s `guide` SENDS people, so its own control
  // is install-or-check and never a third thing pointing back here. Both the
  // label and the click read this one route, so they cannot disagree.
  const offeredVersion = packagedUpdateVersion();
  const canInstallHere = canInstallUpdateHere();
  const canCheckHere = canCheckForUpdatesHere();
  const pageRoute: UpdateRoute = canInstallHere ? 'install' : 'check';
  const tauriHasUpdate = !!offeredVersion;
  const check = releaseCheck.value;
  // How this install takes an update, as the gateway read it from its own
  // executable path. A headless install gets a command to copy; a bundle is
  // installed by the client, and only a client can do it.
  const offer = check?.latest ?? null;
  const updateCommand = offer?.install === 'installer-rerun' ? offer.command : null;
  // An update in flight OWNS this control: the same derivation the progress
  // dialog renders, so the persistent surface and the transient one can never
  // disagree about what the update is doing. Terminal frames clear the signal,
  // so this is non-null exactly while a run is live.
  const updateRun = appUpdateProgress.value;
  const updateNarration = updateRun ? appUpdateNarration(updateRun) : null;
  // A check the USER started, so the button says so and refuses a second. The
  // background poll never sets this, which is what keeps the button still while
  // a resume re-reads the gateway.
  const checking = appUpdateCheckInFlight.value;
  const clientBehind = tauriClientVersion ? tauriHasUpdate : update;
  const clientBehindLabel = tauriClientVersion ? ` (latest: ${offeredVersion})` : ' (update available)';
  // `packaged` reads false until /health answers. A user with a dead engine
  // comes to this very page. So the guidance waits for the engine's own word
  // rather than asserting a source checkout by default.
  const guidance = updateGuidance({
    engineAnswered: status === 'connected',
    packaged: enginePackaged.value,
    hasOffer: tauriHasUpdate,
    sessionCanInstall: sessionCanInstall(),
    canCheckHere,
    install: offer?.install ?? null,
  });

  function renderPanel() {
    switch (panel) {
      case 'release-notices': return <ReleaseNoticesPage />;
      case 'whats-new': return <WhatsNewPage />;
      case 'thread-queue': return <ThreadQueueView />;
      case 'backup': return <BackupSection />;
      case 'memory': return <MemoryInspector />;
      case 'disk-usage': return <DiskUsagePage />;
      case 'environment-variables': return <EnvironmentVariablesPage />;
      case 'debugging': return <DebuggingSection />;
      case 'communication-surfaces': return <CommunicationSurfacesPage />;
      default: return renderOverview();
    }
  }

  function renderOverview() {
    return (
      <>
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="system:connection">Connection</div>
          {/* The state, and what it MEANS for the app. The word alone was this
              page's whole answer, and "Disconnected" beside a red dot says
              nothing the dot had not already said in colour. Both halves come
              from the one notice table (utils/connectionNotice.ts), so this page,
              the Lucidos menu and the header bar cannot make different claims;
              `connected` is deliberately absent from it, which is why that one
              case keeps its own word and has nothing under it.
              The dot is `aria-hidden`: the state is spelled out right beside it. */}
          <div class={`system-status-row${notice ? ' has-note' : ''}`}>
            <span class={`status-dot ${status}`} aria-hidden="true" />
            <span>{notice ? notice.title : 'Connected'}</span>
          </div>
          {notice && <p class="system-status-note">{notice.detail}</p>}
          <div class="system-info-list">
            <div class="system-info-row">
              <span class="system-info-label">Workspace</span>
              <span class="system-info-value">{name || path || 'unknown'}</span>
            </div>
            {path && name && (
              <div class="system-info-row">
                <span class="system-info-label">Path</span>
                <span class="system-info-value system-info-long">{path}</span>
              </div>
            )}
            <div class="system-info-row">
              <span class="system-info-label">API</span>
              <button class="system-info-value system-info-long system-api-url accent-link" onClick={() => copyToClipboard(getApiUrl())}>
                {getApiUrl()}
              </button>
            </div>
          </div>
        </div>

        {/* Locale (language + timezone) and the coding-agent binary paths used
            to render here. Neither is a system concern: Locale is a plain user
            preference and now has its own top-level category, and the binary
            paths moved to Coding Agents beside the repositories they run in.
            What is left is what System actually is: this workspace's
            connection, what version is running, and how to move it forward. */}
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="system:versions">Versions</div>
          <div class="system-info-list">
            {release && (
              <div class="system-info-row">
                <span class="system-info-label">Lucidos</span>
                <span class="system-info-value">
                  {release}
                  {releaseDirty && <span class="system-update"> *</span>}
                </span>
              </div>
            )}
            {engineVer && (
              <div class="system-info-row">
                <span class="system-info-label">Engine</span>
                <span class="system-info-value">
                  {engineVer}
                  {hasEngineUpdate && (
                    <span class="system-update"> (latest: {latestEngineVer})</span>
                  )}
                </span>
              </div>
            )}
            {clientVersion && (
              <div class="system-info-row">
                <span class="system-info-label">Client</span>
                <span class="system-info-value">
                  {clientVersion}
                  {clientBehind && (
                    <span class="system-update">{clientBehindLabel}</span>
                  )}
                </span>
              </div>
            )}
            {swBuildLabel && (
              <div class="system-info-row">
                {/* The build the controlling service worker was installed from —
                    normally identical to Client above, and different exactly
                    while a swap is mid-flight or the worker is wedged. Labelled
                    distinctly so the two build ids can't be read as one. */}
                <span class="system-info-label">Service worker</span>
                <span class="system-info-value">{swBuildLabel}</span>
              </div>
            )}
            {startedAt && (
              <div class="system-info-row">
                <span class="system-info-label">Uptime</span>
                <span class="system-info-value">since {formatShortTime(new Date(startedAt))}</span>
              </div>
            )}
          </div>
          {release && releaseDirty && (
            <div class="system-footnote">
              * code has changed since the {release} release
            </div>
          )}
        </div>

        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="system:maintenance">Maintenance</div>
          {restart && (
            <div class="system-notice">
              {hasEngineUpdate
                ? 'New engine version available - restart to activate'
                : 'Engine changes applied - restart to activate'}
            </div>
          )}
          {update && !restart && (
            <div class="system-notice">
              Client update available - refresh to activate
            </div>
          )}
          {/* The PERSISTENT route to a packaged update. The in-app toast is
              transient and dismissable, so it cannot be the only way to reach
              one. Dismissing it used to strand the user until the next poll
              (or a full quit-and-relaunch). Once a run starts, the same live
              phase the toast narrates replaces the offer here. */}
          {updateNarration ? (
            <div class="system-notice">
              {updateNarration.message}
              {updateNarration.progress !== null && (
                <div class="progress-bar system-update-progress">
                  <div class="progress-bar-fill" style={`width: ${Math.round(updateNarration.progress * 100)}%`} />
                </div>
              )}
            </div>
          ) : tauriHasUpdate && (
            <div class="system-notice">
              {canInstallHere
                ? `Lucidos ${offeredVersion} is available - update and restart to install`
                : `Lucidos ${offeredVersion} is available`}
              {/* What is in it, before deciding to take it. The notes come from
                  the update manifest, so What's New shows THAT release above the
                  installed history; rendered only when the manifest carried
                  them, so the link never opens onto nothing. It names the
                  version, which is what opens the panel on it rather than on the
                  release already running. */}
              {latestTauriAppNotes.value && (
                <>
                  {' '}
                  <button
                    class="accent-link"
                    onClick={() => openWhatsNew(offeredVersion)}
                  >
                    What's new
                  </button>
                </>
              )}
            </div>
          )}
          {/* A headless install updates by re-running the installer, so the
              answer is a command rather than a button. The gateway composes it
              from the live instance, so the slug and the prefix are this
              install's own. It never runs it: on macOS `launchctl bootout`
              tears down the job's whole process group, so a spawned installer
              would kill itself mid-replace (ADR 0108). */}
          {updateCommand && (
            <div class="system-notice">
              Re-run the installer to update:
              {' '}
              <code>{updateCommand}</code>
              {' '}
              <button class="accent-link" onClick={() => copyToClipboard(updateCommand)}>
                Copy
              </button>
            </div>
          )}
          {/* What the controls on this page cannot say for themselves. It is
              here because `updateRoute`'s `guide` lands people on this page
              (ADR 0142), so it owes an answer to every install shape that
              arrives. The rule is `updateGuidance`, which is pure. */}
          {guidance === 'source-checkout' && (
            <div class="system-notice system-guidance">
              This engine runs from a source checkout, so no update is
              downloaded. Pull the release, then Rebuild &amp; Restart.
            </div>
          )}
          {guidance === 'install-in-the-app' && (
            <div class="system-notice system-guidance">
              An update installs from the Lucidos app on the machine that runs
              this workspace, never from a browser session.
            </div>
          )}
          {/* A check that FAILED must not look like "you are up to date". */}
          {appUpdateCheckError.value && (
            <div class="system-notice">
              Couldn't check for updates: {appUpdateCheckError.value}
            </div>
          )}
          <div class="system-actions">
            <button class="action-btn" onClick={handleRefresh}>
              Refresh Client
            </button>
            {/* While a run is live this button must not offer to start another —
                it reports the phase instead, and turns into the same Cancel the
                toast offers for exactly as long as one is honest.

                A LIVE run is its own reason to render, outside the capability
                gate below it. That gate reads signals a background poll also
                writes, so folding the two together would let a mid-run refresh
                take the Cancel away from an install still downloading. */}
            {updateNarration
              ? (updateNarration.cancellable
                  ? <button class="action-btn action-btn-danger" onClick={() => { void cancelAppUpdate(); }}>Cancel Update</button>
                  : <button class="action-btn" disabled>Updating…</button>)
              : (canInstallHere || canCheckHere) && (
                <button
                  class="action-btn"
                  onClick={() => { void followUpdateRoute(pageRoute); }}
                  disabled={checking}
                >
                  {updateControlLabel(pageRoute, checking)}
                </button>
              )}
            {ownsRestart && (
              <button class="action-btn" onClick={() => { void confirmAndRestartEngine(); }}>
                Rebuild &amp; Restart
              </button>
            )}
          </div>
          {/* The machine's release check, beside the thing it governs rather
              than buried. What it sends belongs behind the explainer, never at
              rest on the page (ADR 0159, amending ADR 0139); `PRIVACY.md` is
              the full notice. The copy reads correctly in both switch
              positions, since prose describing an hourly request while the
              switch is off is the one thing it must not do. Rendered only
              where the check can run, so a dev gateway shows no knob for a
              poll it will never make. */}
          {check?.supported && (
            <div class="settings-row" data-search-anchor="system:update-check">
              <span class="settings-row-label">
                Check for updates automatically
                <Explainer title="Check for updates automatically">
                  <p>
                    While this is on, Lucidos asks <code>lucidos.dev</code> once an
                    hour whether a newer version is published. It is a regular web
                    request, and it says which platform, architecture and version
                    you run.
                  </p>
                  <p>
                    One request per machine, from the gateway, however many windows
                    you have open. A gateway started from a source checkout never
                    asks at all.
                  </p>
                  <p>
                    Nothing else is sent, nothing identifies you, and nothing
                    installs itself. Taking an update is always your click.
                  </p>
                  <p>
                    Turn it off and Lucidos stops asking on its own. You can still
                    check by hand with the button above.
                  </p>
                </Explainer>
              </span>
              <label class="toggle-switch">
                <input
                  type="checkbox"
                  checked={check.enabled}
                  onChange={(e) => {
                    const on = (e.currentTarget as HTMLInputElement).checked;
                    void setReleaseCheckEnabled(on);
                  }}
                />
                <span class="toggle-slider" />
              </label>
            </div>
          )}
        </div>
      </>
    );
  }

  // The wrapper is what caps every sub-page at the column width, so it stays
  // even though the switcher it used to carry is gone.
  return <div class="system-page">{renderPanel()}</div>;
}
