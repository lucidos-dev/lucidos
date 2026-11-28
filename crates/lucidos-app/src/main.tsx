import './utils/workspaceStorage.install';
import './utils/pageHealthLog.install';
import { render, type ComponentChildren } from 'preact';
import { lazyComponent, lazyModule } from './utils/lazyComponent';
import { provideMarkdownEngine } from './utils/renderMarkdown';
import { IS_PICKER, REMOTE_GATEWAY, WORKSPACE_ID, baseContextIsValid, clearRemotePreflightQuery, parseWorkspaceRoute } from './utils/basePath';
import { rememberLastWorkspace } from './utils/lastWorkspace';
import { showToast, updateAvailable } from './store/store';
import { installActionBtnBlurListener } from './components/chat/promptFocus';
import { installNoAutofill } from './utils/noAutofill';
import { currentAutocorrect, paintThemeColor } from './store/actions/preferences';
import { installNoDrag } from './utils/noDrag';
import { installNativeContextMenuPolicy } from './utils/nativeContextMenu';
import { installNoKeyCodeText } from './utils/noKeyCodeText';
import { installStrayFileDropGuard } from './utils/strayFileDrop';
import { publishScrollbarGutter } from './utils/scrollbarGutter';
import { installSafeAreaFloor } from './utils/safeAreaFloor';
import { installStatusBarBlurClearance } from './utils/statusBarBlurClearance';
import { isTouchDevice } from './utils/viewport';
import { isIOSPwa, isTauri, isTauriPreGatewayEntry } from './utils/platform';
import { invoke, windowReadyToShow } from './utils/tauri';
import { startPageHeartbeat } from './utils/heartbeat';
import { handOverBootOwnership, setBootAction, setBootStatus } from './utils/bootSplash';
import { startStartupStatusPolling } from './utils/startupStatus';
import { adoptRoutedDeviceId, reconcileDesktopDeviceId, type DeviceRoute } from './store/actions/devices';
import { takePairingCodeFromUrl } from './utils/pairingCodeSeed';
import { openAppById } from './store/actions/apps';
import { startClient } from './store/startup';
import { installMainThreadStallProbe } from './utils/mainThreadStall';
import { OUTBOX_IMPORT_PATH } from './handoff/outboxHandoff';
import { importNativeOutbox, indexedDbImportTarget, startOutboxImportPage } from './handoff/outboxImport';
import { errorDetail } from './utils/errorDetail';
import './styles/global.css';
import './styles/picker.css';
import './styles/header.css';
import './styles/header-mark.css';
import './styles/panels.css';
import './styles/chat.css';
import './styles/steps.css';
import './styles/settings.css';
import './styles/components.css';
import './styles/pages.css';
import './styles/skills.css';
import './styles/thread-queue.css';
import './styles/mobile.css';
import './styles/section-header.css';
import './styles/drawer.css';
import './styles/badges.css';
import './store/actions/wipPreview';

const hmrTeardowns: (() => void)[] = [];
import.meta.hot?.dispose(() => {
  for (const stop of hmrTeardowns.splice(0)) stop();
});

const App = lazyComponent(() =>
  import('./App').then((m) => {
    hmrTeardowns.push(m.startShell());
    handOverBootOwnership();
    return m.App;
  }),
);
const markdownEngine = lazyModule(() => import('./utils/markdownEngine'));
if (!IS_PICKER && !isTauriPreGatewayEntry()) {
  void App.preload();
  void markdownEngine.preload().then(async (loaded) => {
    if (loaded && await markdownEngine.get().loadMarkdownLibraries()) {
      provideMarkdownEngine(markdownEngine.get());
    }
  });
}

if (isTouchDevice()) {
  document.body.classList.add('is-touch');
}

if (isIOSPwa()) {
  document.documentElement.classList.add('ios-pwa');
}
paintThemeColor();
installSafeAreaFloor();
installStatusBarBlurClearance();

installActionBtnBlurListener();
installNoAutofill(currentAutocorrect());
installNoDrag();
installNativeContextMenuPolicy();
installNoKeyCodeText();
installStrayFileDropGuard();
publishScrollbarGutter();

(window as unknown as { __openApp?: (id: string) => Promise<boolean> }).__openApp =
  openAppById;

const appRoot = document.getElementById('app')!;

const PairingGate = lazyComponent<{ children: ComponentChildren }>(() =>
  import('./components/picker/PairingGate').then((m) => m.PairingGate),
);

const WorkspacePicker = lazyComponent(() =>
  import('./components/picker/WorkspacePicker').then((m) => {
    handOverBootOwnership();
    return m.WorkspacePicker;
  }),
);

const RECOVER_REDIRECT_KEY = 'lucidos-recover-redirect';
function recoverFromBrokenContext(): boolean {
  if (WORKSPACE_ID === null || baseContextIsValid()) {
    try { sessionStorage.removeItem(RECOVER_REDIRECT_KEY); } catch {  }
    return false;
  }
  let alreadyTried = false;
  try { alreadyTried = sessionStorage.getItem(RECOVER_REDIRECT_KEY) === '1'; } catch {  }
  if (alreadyTried) return false;
  try { sessionStorage.setItem(RECOVER_REDIRECT_KEY, '1'); } catch {  }
  location.replace('/~/?pick');
  return true;
}

function stayOnStartingSplash(): void {
  handOverBootOwnership();
  setBootStatus('Starting Lucidos…');
  startPageHeartbeat();
  startStartupStatusPolling({
    invoke,
    setStatus: setBootStatus,
    setAction: setBootAction,
    setInterval: (fn, ms) => window.setInterval(fn, ms),
  });
}

async function boot() {
  const launch = window as Window & {
    lucidosRemoteClientReady?: Promise<void>;
    lucidosWorkspaceResume?: boolean;
    lucidosRememberPwaWorkspace?: (route: string) => void;
  };
  await launch.lucidosRemoteClientReady;
  if (launch.lucidosWorkspaceResume) return;
  if (IS_PICKER && location.pathname === OUTBOX_IMPORT_PATH) {
    handOverBootOwnership();
    startOutboxImportPage({
      opener: window.opener as Window | null,
      addMessageListener: (listener) => window.addEventListener('message', listener),
      port: location.port,
      target: indexedDbImportTarget(),
      report: setBootStatus,
      finish: () => location.replace('/~/'),
    });
    return;
  }

  takePairingCodeFromUrl();

  if (isTauri()) windowReadyToShow();

  if (isTauriPreGatewayEntry()) {
    stayOnStartingSplash();
    return;
  }

  const deviceRoute: DeviceRoute = {
    workspace: WORKSPACE_ID ?? null,
    relayed: Boolean(WORKSPACE_ID && parseWorkspaceRoute(WORKSPACE_ID)?.gateway),
  };
  const namedByGateway = IS_PICKER ? false : await adoptRoutedDeviceId(deviceRoute);

  if (isTauri() && !IS_PICKER && WORKSPACE_ID && !namedByGateway) {
    await reconcileDesktopDeviceId(WORKSPACE_ID);
  }

  if (isTauri()) {
    await importNativeOutbox(invoke, indexedDbImportTarget()).catch((err: unknown) => {
      showToast(`Could not restore the unsent work saved at the old address (${errorDetail(err)}). It stays saved, and Lucidos tries again next time it starts.`, 'error');
    });
  }

  if (!recoverFromBrokenContext()) {
    if (REMOTE_GATEWAY !== null) clearRemotePreflightQuery();
    if (!IS_PICKER && WORKSPACE_ID) {
      rememberLastWorkspace(WORKSPACE_ID);
      if (!isTauri()) launch.lucidosRememberPwaWorkspace?.(WORKSPACE_ID);
    }
    if (!IS_PICKER) {
      import('./utils/perfProbe')
        .then(({ startPerfProbe }) => startPerfProbe())
        .catch((e: unknown) => console.warn('[perf] probe chunk failed to load', e));
      installMainThreadStallProbe();
      hmrTeardowns.push(startClient());
    }
    render(
      IS_PICKER ? (
        <PairingGate>
          <WorkspacePicker />
        </PairingGate>
      ) : (
        <App />
      ),
      appRoot,
    );
  }
}
void boot().catch((error: unknown) => {
  handOverBootOwnership();
  setBootStatus(error instanceof Error ? error.message : 'Could not update remote workspace access. Retry from the workspace picker.');
});

if (import.meta.hot) {
  import.meta.hot.accept();

  import.meta.hot.on('lucidos:update-available', () => {
    updateAvailable.value = true;
  });

  import.meta.hot.on('vite:beforeFullReload', () => {
    updateAvailable.value = true;
    throw 'suppress-reload';
  });

  import.meta.hot.on('vite:ws:disconnect', () => {
    throw 'suppress-reload';
  });
}
