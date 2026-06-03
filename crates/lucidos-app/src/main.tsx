import { render } from 'preact';
import { App } from './App';
import { updateAvailable } from './store/store';
import { installActionBtnBlurListener } from './components/chat/promptFocus';
import { isTouchDevice } from './utils/viewport';
import { openAppById } from './store/actions/apps';
import './styles/global.css';
import './styles/header.css';
import './styles/panels.css';
import './styles/chat.css';
import './styles/steps.css';
import './styles/settings.css';
import './styles/components.css';
import './styles/pages.css';
import './styles/skills.css';
import './styles/mobile.css';
import './styles/drawer.css';
import './store/effects';
import './store/actions/wipPreview';

if (isTouchDevice()) {
  document.body.classList.add('is-touch');
}

installActionBtnBlurListener();

// E2E test hook — Playwright opens an app by id from `page.evaluate`. The
// real `openApp(app: App)` requires an `App` object that the test doesn't
// hold, and the production `openAppById` lives behind an ES import the
// browser can't reach from `page.evaluate`. Gated on non-production builds
// so the hook ships only in dev/test bundles — Lucidos's e2e runs against
// the Vite dev server (`MODE === 'development'`) while the desktop release
// build is `'production'`.
if (import.meta.env.MODE !== 'production') {
  (window as unknown as { __openApp?: (id: string) => Promise<void> }).__openApp =
    openAppById;
}

render(<App />, document.getElementById('app')!);

if (import.meta.hot) {
  import.meta.hot.accept();

  // Server-side suppressMergeReload plugin detects git-merge file bursts and
  // drops the HMR update before it reaches the client. It sends this custom
  // event so the client can show an "update available" indicator.
  import.meta.hot.on('lucidos:update-available', () => {
    updateAvailable.value = true;
  });

  // Always suppress Vite full-reloads — show an "update available" indicator
  // instead. Full-reloads lose UI state and, because all Vite instances share
  // the same source directory, a rebuild in one workspace triggers reloads in
  // every workspace's browser tab. Suppressing unconditionally keeps all tabs
  // stable; the user reloads manually via the refresh button when ready.
  import.meta.hot.on('vite:beforeFullReload', () => {
    updateAvailable.value = true;
    throw 'suppress-reload';
  });

  // Suppress Vite's auto-reload on WebSocket disconnect. When the engine
  // restarts, the Vite dev server also restarts. Vite's client polls until
  // Vite comes back, then calls location.reload() directly (bypassing
  // vite:beforeFullReload). This reload hits the engine port before the
  // engine is ready, causing a "Can't connect to localhost" browser error.
  // Throwing here prevents that reload — the health-check polling in
  // connection.ts handles reconnection gracefully with the red status dot.
  import.meta.hot.on('vite:ws:disconnect', () => {
    throw 'suppress-reload';
  });
}
