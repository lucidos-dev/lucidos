/**
 * Keeping an open app frame's URL pass alive.
 *
 * An app frame's opaque origin sends no device credential. So behind a gateway
 * its own files load with a short-lived pass in the URL (ADR 0238). The engine
 * stamps the first one into the document. A pass lasts an hour and an app stays
 * open for longer, so somebody has to re-mint.
 *
 * The HOST does it, because the host is the side that holds the device
 * credential and a real origin. An app asks for nothing and notices nothing:
 * the SDK swaps the new token into its `<base href>` and every later relative
 * ref picks it up. Nothing reloads.
 *
 * The app id comes from the frame's `src`, which the host set, never from
 * anything the app said (ADR 0231 decision 4).
 */

import { API } from '../../api/client';
import { deviceIdHeader } from '../../utils/deviceIdHeader';
import { appIdForFrame } from '../../utils/appFrame';
import { postToAppFrame } from './app-bridge';

/** Half the engine's TTL. Replaced by what the engine answers. */
const DEFAULT_RENEW_AFTER_MS = 30 * 60 * 1000;

/** After a failed round, before trying again. Short, because a lapsed pass is
 *  a visibly broken app and the engine is usually a reload away from back. */
const RETRY_AFTER_MS = 60 * 1000;

let timer: ReturnType<typeof setTimeout> | null = null;
let running = false;

/**
 * Start the renewal loop. Idempotent, so a second call is a no-op.
 *
 * One timer for every open frame rather than one each. A round mints per app,
 * and there are rarely more than a handful mounted.
 */
export function startFrameCapabilityRenewal(): void {
  if (running) return;
  running = true;
  schedule(DEFAULT_RENEW_AFTER_MS);
}

/**
 * Stop it. For tests, and for a shell tearing itself down.
 *
 * `running` is what makes the stop stick. A round already in flight finishes
 * after the timer is cleared, and it would otherwise arm the next one behind
 * the teardown's back.
 */
export function stopFrameCapabilityRenewal(): void {
  running = false;
  if (timer !== null) clearTimeout(timer);
  timer = null;
}

function schedule(delayMs: number): void {
  if (timer !== null) clearTimeout(timer);
  timer = setTimeout(() => {
    void tick();
  }, delayMs);
}

/** One round, then the next timer, for as long as the loop is running. */
async function tick(): Promise<void> {
  const nextDelayMs = await renewEveryOpenFrame();
  if (running) schedule(nextDelayMs);
}

/**
 * Hand every mounted app frame a fresh pass, and say when to come back.
 *
 * Exported for the test, and it schedules nothing itself. A frame the engine
 * minted nothing for ignores the push, so this needs no idea which frames are
 * behind a gateway.
 */
export async function renewEveryOpenFrame(): Promise<number> {
  const frames = document.querySelectorAll<HTMLIFrameElement>('iframe[data-role="app-ui-frame"]');
  let nextDelayMs = DEFAULT_RENEW_AFTER_MS;
  let failed = false;
  for (const frame of frames) {
    const appId = appIdForFrame(frame);
    if (!appId) continue;
    try {
      const minted = await mintFor(appId);
      nextDelayMs = minted.renewAfterMs;
      if (minted.capability) {
        postToAppFrame(frame, 'frame-capability', { capability: minted.capability });
      }
    } catch (err) {
      failed = true;
      // Telemetry, not a toast. Nobody asked for this round: it runs on a
      // timer, and the retry below re-runs it in a minute. A pass that really
      // lapses surfaces where it matters, as the app's own 401 on the file it
      // asked for. The gateway's refusal page names the reload that fixes it.
      console.warn(`[app-bridge] could not renew the pass for "${appId}":`, err);
    }
  }
  return failed ? RETRY_AFTER_MS : nextDelayMs;
}

interface MintedCapability {
  capability: string | null;
  renewAfterMs: number;
}

async function mintFor(appId: string): Promise<MintedCapability> {
  const url = `${API}/app-frame-capability?app_id=${encodeURIComponent(appId)}`;
  const res = await fetch(url, { headers: deviceIdHeader() });
  if (!res.ok) throw new Error(`the engine answered ${res.status}`);
  const body = await res.json() as { capability?: unknown; renew_after_secs?: unknown };
  const seconds = typeof body.renew_after_secs === 'number' && body.renew_after_secs > 0
    ? body.renew_after_secs
    : DEFAULT_RENEW_AFTER_MS / 1000;
  return {
    capability: typeof body.capability === 'string' ? body.capability : null,
    renewAfterMs: seconds * 1000,
  };
}
