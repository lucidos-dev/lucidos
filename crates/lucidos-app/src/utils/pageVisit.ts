
type Listener = () => void;

const wakeListeners = new Set<Listener>();
const hideListeners = new Set<Listener>();

let installed = false;
let away = false;
let hides = 0;
let watched = false;

function fire(listeners: Set<Listener>): void {
  for (const cb of [...listeners]) cb();
}

function goAway(): void {
  if (away) return;
  away = true;
  hides += 1;
  fire(hideListeners);
}

function comeBack(): void {
  if (!away) return;
  away = false;
  fire(wakeListeners);
}

function onVisibilityChange(): void {
  if (typeof document === 'undefined') return;
  if (document.visibilityState === 'hidden') goAway();
  else comeBack();
}

let boundDoc: typeof document | null = null;
let boundWin: typeof window | null = null;

function install(): void {
  if (installed) return;
  if (typeof document === 'undefined' || typeof window === 'undefined') return;
  if (typeof document.addEventListener !== 'function') return;
  if (typeof window.addEventListener !== 'function') return;
  installed = true;
  boundDoc = document;
  boundWin = window;
  away = document.visibilityState === 'hidden';
  boundDoc.addEventListener('visibilitychange', onVisibilityChange);
  boundWin.addEventListener('pagehide', goAway);
  boundWin.addEventListener('pageshow', comeBack);
  boundWin.addEventListener('focus', comeBack);
}

function uninstallIfIdle(): void {
  if (!installed || watched || wakeListeners.size > 0 || hideListeners.size > 0) return;
  installed = false;
  boundDoc?.removeEventListener('visibilitychange', onVisibilityChange);
  boundWin?.removeEventListener('pagehide', goAway);
  boundWin?.removeEventListener('pageshow', comeBack);
  boundWin?.removeEventListener('focus', comeBack);
  boundDoc = null;
  boundWin = null;
}

function subscribe(listeners: Set<Listener>, cb: Listener): () => void {
  install();
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
    uninstallIfIdle();
  };
}

export function onPageWake(cb: Listener): () => void {
  return subscribe(wakeListeners, cb);
}

export function onPageHide(cb: Listener): () => void {
  return subscribe(hideListeners, cb);
}

export function watchPageAway(): () => boolean {
  watched = true;
  install();
  const awayAtStart = away;
  const hidesAtStart = hides;
  return () => awayAtStart || away || hides !== hidesAtStart;
}

export function _resetPageVisitForTesting(): void {
  wakeListeners.clear();
  hideListeners.clear();
  watched = false;
  uninstallIfIdle();
  away = false;
}
