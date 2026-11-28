import { retryTransportRead } from '../../../../../packages/lucidos-sdk/src/_transport';

import { signal } from '@preact/signals';
import { API } from '../../api/client';
import { deviceIdHeader } from '../../utils/deviceIdHeader';
import { appIdForFrame, mountedAppFrames } from '../../utils/appFrame';
import { PREVIEW_FRAME_ROLE, postToPreviewFrame } from '../../utils/previewFrameProtocol';
import { CAPABILITY_SEGMENT } from '../../../../../packages/lucidos-sdk/src/frameCapability';
import { postToAppFrame } from './app-frame-channel';

const DEFAULT_RENEW_AFTER_MS = 30 * 60 * 1000;

const RETRY_AFTER_MS = 60 * 1000;

let timer: ReturnType<typeof setTimeout> | null = null;
let running = false;

export function startFrameCapabilityRenewal(): void {
  if (running) return;
  running = true;
  schedule(DEFAULT_RENEW_AFTER_MS);
}

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

async function tick(): Promise<void> {
  const nextDelayMs = await renewEveryOpenFrame();
  if (running) schedule(nextDelayMs);
}

export async function renewEveryOpenFrame(): Promise<number> {
  const frames = mountedAppFrames();
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
      console.warn('[preview] could not renew the artifact preview pass:', err);
    }
  }
  return failed ? RETRY_AFTER_MS : nextDelayMs;
}

const previewCapability = signal<{ capability: string | null; freshUntilMs: number } | null>(null);
let previewCapabilityInFlight: Promise<string | null> | null = null;

export function artifactPreviewCapability(): Promise<string | null> {
  const cached = previewCapability.peek();
  if (cached && cached.freshUntilMs > Date.now()) return Promise.resolve(cached.capability);
  previewCapabilityInFlight ??= mintArtifactPreviewCapability()
    .then((minted) => minted.capability)
    .finally(() => { previewCapabilityInFlight = null; });
  return previewCapabilityInFlight;
}

export function currentArtifactPreviewCapability(): string | null {
  return previewCapability.value?.capability ?? null;
}

export function peekArtifactPreviewCapability(): string | null {
  return previewCapability.peek()?.capability ?? null;
}

export function withPreviewCapability(fileUrl: string, capability: string | null): string {
  if (!capability) return fileUrl;
  const at = fileUrl.indexOf('/data/artifacts/');
  if (at === -1 || fileUrl.slice(0, at).endsWith('/api/v1')) return fileUrl;
  return `${fileUrl.slice(0, at)}/${CAPABILITY_SEGMENT}/${capability}${fileUrl.slice(at)}`;
}

async function mintArtifactPreviewCapability(): Promise<MintedCapability> {
  const minted = await mintAt(`${API}/artifact-preview-capability`);
  previewCapability.value = { capability: minted.capability, freshUntilMs: Date.now() + minted.renewAfterMs };
  return minted;
}

export function resetArtifactPreviewCapability(): void {
  previewCapability.value = null;
  previewCapabilityInFlight = null;
}

interface MintedCapability {
  capability: string | null;
  renewAfterMs: number;
}

async function mintAt(url: string): Promise<MintedCapability> {
  const res = await retryTransportRead(() => fetch(url, { headers: deviceIdHeader() }));
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
