/**
 * Keeping an open frame's URL pass alive: an app frame's, and the HTML
 * artifact preview frame's.
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
 *
 * The HTML artifact preview frame is opaque-origin too (ADR 0322), so it
 * carries a pass of its own, narrowed to the artifacts tree. The same round
 * renews it and pushes it into every mounted preview.
 */

import { signal } from '@preact/signals';
import { API } from '../../api/client';
import { deviceIdHeader } from '../../utils/deviceIdHeader';
import { appIdForFrame } from '../../utils/appFrame';
import { PREVIEW_FRAME_ROLE, postToPreviewFrame } from '../../utils/previewFrameProtocol';
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
 * Hand every mounted app frame and preview frame a fresh pass, and say when to
 * come back.
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
      const minted = await mintAt(`${API}/app-frame-capability?app_id=${encodeURIComponent(appId)}`);
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
  const previews = document.querySelectorAll<HTMLIFrameElement>(`iframe[data-role="${PREVIEW_FRAME_ROLE}"]`);
  if (previews.length > 0) {
    try {
      const minted = await mintArtifactPreviewCapability();
      nextDelayMs = Math.min(nextDelayMs, minted.renewAfterMs);
      if (minted.capability) {
        for (const preview of previews) {
          postToPreviewFrame(preview.contentWindow, { kind: 'capability', capability: minted.capability });
        }
      }
    } catch (err) {
      failed = true;
      // Telemetry, for the reason above: a timer round, retried in a minute.
      // A lapsed pass shows as the preview's own broken image.
      console.warn('[preview] could not renew the artifact preview pass:', err);
    }
  }
  return failed ? RETRY_AFTER_MS : nextDelayMs;
}

/** The latest preview pass, and until when it is worth handing out. */
const previewCapability = signal<{ capability: string | null; freshUntilMs: number } | null>(null);
let previewCapabilityInFlight: Promise<string | null> | null = null;

/**
 * The pass an HTML artifact preview stamps into its `<base href>`, or `null`
 * with no gateway in front, where nothing needs proving.
 *
 * Cached for half its life, the same half-life the renewal loop runs on. So a
 * new preview always gets a pass that outlives the next renewal round, and a
 * second preview costs no round trip.
 */
export function artifactPreviewCapability(): Promise<string | null> {
  const cached = previewCapability.peek();
  if (cached && cached.freshUntilMs > Date.now()) return Promise.resolve(cached.capability);
  previewCapabilityInFlight ??= mintArtifactPreviewCapability()
    .then((minted) => minted.capability)
    .finally(() => { previewCapabilityInFlight = null; });
  return previewCapabilityInFlight;
}

/** The latest preview pass, read reactively. For a URL the host builds, such as
 *  the header's "Open in new tab" link. */
export function currentArtifactPreviewCapability(): string | null {
  return previewCapability.value?.capability ?? null;
}

/** The latest preview pass, without subscribing. A preview stamps it when it
 *  builds its document, and must not rebuild because a renewal landed: the
 *  renewal reaches the live document over the bridge instead. */
export function peekArtifactPreviewCapability(): string | null {
  return previewCapability.peek()?.capability ?? null;
}

async function mintArtifactPreviewCapability(): Promise<MintedCapability> {
  const minted = await mintAt(`${API}/artifact-preview-capability`);
  previewCapability.value = { capability: minted.capability, freshUntilMs: Date.now() + minted.renewAfterMs };
  return minted;
}

/** Forget the cached preview pass. For tests. */
export function resetArtifactPreviewCapability(): void {
  previewCapability.value = null;
  previewCapabilityInFlight = null;
}

interface MintedCapability {
  capability: string | null;
  renewAfterMs: number;
}

async function mintAt(url: string): Promise<MintedCapability> {
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
