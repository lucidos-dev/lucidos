import { render } from 'preact';
import { App } from './App';
import { updateAvailable } from './store/store';
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

render(<App />, document.getElementById('app')!);

if (import.meta.hot) {
  import.meta.hot.accept();

  // Server-side suppressMergeReload plugin detects git-merge file bursts and
  // drops the HMR update before it reaches the client. It sends this custom
  // event so the client can show an "update available" indicator.
  import.meta.hot.on('cognos:update-available', () => {
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
