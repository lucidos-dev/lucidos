/** The transport starts before paint; its presenter can arrive with the shell. */
import { listen, APP_UPDATE_PROGRESS_EVENT, type AppUpdateProgress } from '../../utils/tauri';
import { isTauri } from '../../utils/platform';

let receive: ((frame: AppUpdateProgress) => void) | undefined;
const pending: AppUpdateProgress[] = [];
let unlisten: (() => void) | undefined;
let subscribing = false;
let epoch = 0;

export function setAppUpdateProgressHandler(handler: (frame: AppUpdateProgress) => void): void {
  receive = handler;
  for (const frame of pending.splice(0)) handler(frame);
}

/** Best-effort startup telemetry: the install action reports its own failures.
 * A failed listener remains unsubscribed, so the next start can try again. */
export function startAppUpdateProgress(): void {
  if (!isTauri() || unlisten || subscribing) return;
  subscribing = true;
  const started = epoch;
  void listen<AppUpdateProgress>(APP_UPDATE_PROGRESS_EVENT, (event) => {
    if (started !== epoch) return;
    if (receive) receive(event.payload);
    else pending.push(event.payload);
  }).then((stop) => {
    if (started !== epoch) stop();
    else unlisten = stop;
  }).catch((error) => {
    if (started === epoch) console.warn('[app-update] progress subscription failed; retried on next mount', error);
  }).finally(() => { if (started === epoch) subscribing = false; });
}

export function stopAppUpdateProgress(): void {
  epoch++;
  subscribing = false;
  pending.length = 0;
  unlisten?.();
  unlisten = undefined;
}
