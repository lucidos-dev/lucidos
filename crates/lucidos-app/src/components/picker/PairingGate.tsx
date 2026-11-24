
import { useEffect, useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import {
  isPairingSecret,
  releasePairingLaunch,
  PAIRING_CODE_DIGITS,
  takePairingCodeFromUrl,
  takeUnspentPairingCodeFromUrl,
} from '../../utils/pairingCodeSeed';
import { pairingCodeFromText } from '../../utils/pairingCodeText';
import { suggestDeviceLabelHere } from '../../utils/deviceLabel';
import { pairDesktopWindow, redeemPairingCode } from '../../api/client/pairing';
import {
  cameraIsAvailable,
  clipboardAbilities,
  isAndroid,
  isIOS,
  isStandalone,
  isTauri,
  thisDeviceIsMobile,
} from '../../utils/platform';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { dismissBootSplash, handOverBootOwnership } from '../../utils/bootSplash';
import { lazyComponent } from '../../utils/lazyComponent';

const PairingScanner = lazyComponent<{ onCode: (code: string) => void }>(
  () => import('./PairingScanner'),
);

const SESSION_URL = '/~/api/v1/auth/session';

type SessionState = 'checking' | 'paired' | 'unpaired' | 'unreachable';

interface SessionBody {
  paired: boolean;
}

function useUncoverOnPaint(painting: boolean) {
  useEffect(() => {
    if (painting) dismissBootSplash();
  }, [painting]);
}

export function PairingGate({ children }: { children: ComponentChildren }) {
  const [state, setState] = useState<SessionState>('checking');

  useEffect(() => {
    let live = true;
    fetch(SESSION_URL, { credentials: 'same-origin' })
      .then((r) => (r.ok ? (r.json() as Promise<SessionBody>) : null))
      .then((body) => {
        if (!live) return;
        if (!body) setState('unreachable');
        else {
          if (body.paired) releasePairingLaunch();
          setState(body.paired ? 'paired' : 'unpaired');
        }
      })
      .catch(() => live && setState('unreachable'));
    return () => {
      live = false;
    };
  }, []);

  useEffect(() => {
    if (state === 'unpaired') handOverBootOwnership();
  }, [state]);

  if (state === 'checking') return null;
  if (state === 'unpaired') {
    return <DesktopPairing onPaired={() => {
      releasePairingLaunch();
      setState('paired');
    }} />;
  }
  return <>{children}</>;
}

function AwaitingOwnerScreen() {
  useUncoverOnPaint(true);
  return (
    <div class="pairing-gate">
      <div class="pairing-column" role="status">
        <h1 class="pairing-title">Waiting for your Allow</h1>
        <p class="pairing-lede">
          A device is already paired with this Lucidos. On it, open{' '}
          <strong>Settings → Access</strong> and tap <strong>Allow</strong>. This screen
          moves on by itself.
        </p>
        <p class="pairing-lede">
          No paired device works any more? Run <code>lucidos devices revoke --all</code> on
          the machine running Lucidos, then pair again.
        </p>
      </div>
    </div>
  );
}

function DesktopPairing({ onPaired }: { onPaired: () => void }) {
  const [auto, setAuto] = useState<'running' | 'waiting' | 'failed'>(isTauri() ? 'running' : 'failed');
  const [autoError, setAutoError] = useState<string | null>(null);
  const showBusy = useDelayedFlag(auto === 'running');

  useUncoverOnPaint(auto === 'running' && showBusy);

  useEffect(() => {
    if (auto !== 'running') return;
    let live = true;
    pairDesktopWindow(() => live && setAuto('waiting'))
      .then(() => {
        if (!live) return;
        onPaired();
        window.location.reload();
      })
      .catch((e: unknown) => {
        if (!live) return;
        setAutoError(e instanceof Error ? e.message : 'This Mac could not pair itself.');
        setAuto('failed');
      });
    return () => {
      live = false;
    };
  }, []);

  if (auto === 'waiting') return <AwaitingOwnerScreen />;
  if (auto === 'running') {
    if (!showBusy) return null;
    return (
      <div class="pairing-gate">
        <div class="pairing-column">
          <h1 class="pairing-title">Pairing this Mac…</h1>
          <p class="pairing-lede">Lucidos is letting this window in.</p>
        </div>
      </div>
    );
  }
  return <UnpairedScreen onPaired={onPaired} notice={autoError} />;
}

export type PairingScreenBranch = 'install' | 'form';

export function pairingScreenBranch(env: {
  mobile: boolean;
  standalone: boolean;
}): PairingScreenBranch {
  return env.mobile && !env.standalone ? 'install' : 'form';
}

function UnpairedScreen({ onPaired, notice }: { onPaired: () => void; notice?: string | null }) {
  const [branch] = useState(() =>
    pairingScreenBranch({ mobile: thisDeviceIsMobile(), standalone: isStandalone() }),
  );
  const [pairHere, setPairHere] = useState(false);
  if (branch === 'install' && !pairHere) {
    return <InstallFirst onPairHere={() => setPairHere(true)} />;
  }
  return <PairingForm onPaired={onPaired} notice={notice} />;
}

export type InstallPlatform = 'ios' | 'android' | 'other';

export function installPlatformOf(device: { ios: boolean; android: boolean }): InstallPlatform {
  if (device.ios) return 'ios';
  if (device.android) return 'android';
  return 'other';
}

export function installSteps(platform: InstallPlatform): string[] {
  switch (platform) {
    case 'ios':
      return [
        'Tap the Share button at the bottom of Safari.',
        'Choose "Add to Home Screen", then Add.',
        'Open Lucidos from your home screen.',
      ];
    case 'android':
      return [
        'Open the browser menu.',
        'Choose "Install app" or "Add to Home screen".',
        'Open Lucidos from your home screen.',
      ];
    case 'other':
      return [
        'Install Lucidos from your browser, using the install control it offers.',
        'Open Lucidos from where it installed itself.',
      ];
  }
}

function InstallFirst({ onPairHere }: { onPairHere: () => void }) {
  const [scanned] = useState(() => takePairingCodeFromUrl());
  const platform = installPlatformOf({ ios: isIOS(), android: isAndroid() });
  const [copied, setCopied] = useState(false);
  const canCopy = clipboardAbilities().copy;

  useUncoverOnPaint(true);

  const copy = () => {
    if (!scanned) return;
    navigator.clipboard.writeText(scanned).then(
      () => setCopied(true),
      () => setCopied(false),
    );
  };

  return (
    <div class="pairing-gate">
      <div class="pairing-column">
        <h1 class="pairing-title">Install Lucidos first</h1>
        <p class="pairing-lede">
          Your phone treats the app on your home screen as its own device, separate from this
          browser. Pairing here would let the browser in and leave the app locked out.
        </p>
        <ol class="pairing-steps">
          {installSteps(platform).map((step) => (
            <li key={step}>{step}</li>
          ))}
        </ol>
        {scanned && (
          <p class="pairing-lede">
            It pairs itself when it opens, with nothing to type. Do it soon: this code lasts
            five minutes, and after that the app asks you to scan the QR again.
          </p>
        )}
        {scanned && (
          <div class="pairing-aside">
            <p class="pairing-aside-title">Already have it on your home screen?</p>
            <p class="pairing-aside-body">
              Then it cannot pick the code up from here. Copy it, open Lucidos, and paste it on
              the pairing screen. You can also tap Scan there and point the camera at the QR.
            </p>
            <div class="pairing-code-readout">{scanned}</div>
            {canCopy && (
              <button class="pairing-secondary" type="button" onClick={copy}>
                {copied ? 'Copied' : 'Copy code'}
              </button>
            )}
          </div>
        )}
        <button class="pairing-escape" type="button" onClick={onPairHere}>
          Pair this browser instead
        </button>
      </div>
    </div>
  );
}

export interface CodeSlot {
  digit: string;
  active: boolean;
  caret: boolean;
}

export function digitsOnly(raw: string): string {
  return raw.replace(/\D/g, '').slice(0, PAIRING_CODE_DIGITS);
}

export function applyCodeInput(el: { value: string }): string {
  const clean = digitsOnly(el.value);
  if (el.value !== clean) el.value = clean;
  return clean;
}

function caretToEnd(e: Event) {
  const el = e.target as HTMLInputElement;
  el.setSelectionRange(el.value.length, el.value.length);
}

export function codeSlots(code: string, length = PAIRING_CODE_DIGITS): CodeSlot[] {
  const cursor = Math.min(code.length, length - 1);
  return Array.from({ length }, (_, i) => ({
    digit: code[i] ?? '',
    active: i === cursor,
    caret: i === cursor && !code[i],
  }));
}

export function PairingCodeBoxes({ code }: { code: string }) {
  return (
    <div class="pairing-code-boxes" aria-hidden="true">
      {codeSlots(code).map((slot, i) => (
        <div
          key={i}
          class="pairing-code-box"
          data-filled={slot.digit ? 'true' : undefined}
          data-active={slot.active ? 'true' : undefined}
          data-caret={slot.caret ? 'true' : undefined}
        >
          {slot.digit}
        </div>
      ))}
    </div>
  );
}

export function onPhoneCodeSource(standalone: boolean): string | null {
  if (!standalone) return null;
  return (
    'No other machine to hand? Open the same address in your phone browser. ' +
    'If it is still paired, Settings → Access mints a code you can copy and paste here.'
  );
}

function PairingForm({ onPaired, notice }: { onPaired: () => void; notice?: string | null }) {
  const [scanned] = useState(() => takeUnspentPairingCodeFromUrl());
  const [onPhoneSource] = useState(() => onPhoneCodeSource(isStandalone()));
  const [code, setCode] = useState(scanned && !isPairingSecret(scanned) ? scanned : '');
  const [label, setLabel] = useState(() => suggestDeviceLabelHere() ?? '');
  const [error, setError] = useState<string | null>(notice ?? null);
  const [busy, setBusy] = useState(false);
  const [awaitingOwner, setAwaitingOwner] = useState(false);
  const [canPaste] = useState(() => clipboardAbilities().paste);
  const [canScan] = useState(() => thisDeviceIsMobile() && cameraIsAvailable());
  const [scanning, setScanning] = useState(false);
  const [autoPair, setAutoPair] = useState<'running' | 'done'>(scanned ? 'running' : 'done');
  const showAutoPair = useDelayedFlag(autoPair === 'running');
  useUncoverOnPaint(autoPair === 'done' || showAutoPair);

  const paste = async () => {
    let text: string;
    try {
      text = await navigator.clipboard.readText();
    } catch {
      setError('Your phone did not let Lucidos read the clipboard. Type the code instead.');
      return;
    }
    const pasted = pairingCodeFromText(text);
    if (!pasted) {
      setError('The clipboard does not hold a pairing code.');
      return;
    }
    setError(null);
    adopt(pasted);
  };

  const redeem = async (value: string): Promise<boolean> => {
    if (busy) return false;
    setBusy(true);
    setError(null);
    try {
      await redeemPairingCode(value, label.trim() || undefined, () => setAwaitingOwner(true));
      onPaired();
      window.location.reload();
      return true;
    } catch (e) {
      setError(e instanceof Error ? e.message : 'That code was not accepted.');
      return false;
    } finally {
      setBusy(false);
      setAwaitingOwner(false);
    }
  };

  const adopt = (token: string) => {
    if (isPairingSecret(token)) void redeem(token);
    else setCode(token);
  };

  const submit = (e: Event) => {
    e.preventDefault();
    void redeem(code);
  };

  useEffect(() => {
    if (scanned) {
      void redeem(scanned).then((paired) => {
        if (!paired) setAutoPair('done');
      });
    }
  }, []);

  if (awaitingOwner) return <AwaitingOwnerScreen />;

  if (autoPair === 'running') {
    if (!showAutoPair) return null;
    return (
      <div class="pairing-gate">
        <div class="pairing-column">
          <h1 class="pairing-title">Pairing this device…</h1>
          <p class="pairing-lede">Lucidos is letting this app in, with the code it launched with.</p>
        </div>
      </div>
    );
  }

  if (scanning) {
    return (
      <div class="pairing-gate">
        <div class="pairing-column">
          <h1 class="pairing-title">Scan the QR</h1>
          <p class="pairing-lede">
            Point the camera at the QR under Settings → Access on the machine running Lucidos.
          </p>
          <PairingScanner
            onCode={(scannedCode) => {
              setError(null);
              setScanning(false);
              adopt(scannedCode);
            }}
          />
          <button class="pairing-secondary" type="button" onClick={() => setScanning(false)}>
            Cancel
          </button>
        </div>
      </div>
    );
  }

  return (
    <div class="pairing-gate">
      <form class="pairing-column" onSubmit={submit}>
        <h1 class="pairing-title">Pair this device</h1>
        <p class="pairing-lede">
          {scanned
            ? 'Lucidos only answers devices you have paired. The code from the QR came in with this launch. If it is refused, it has expired: make a fresh one under Pair a device on the other machine, then scan again below.'
            : 'Lucidos only answers devices you have paired. Here is where to get a code.'}
        </p>
        {!scanned && (
          <ol class="pairing-steps">
            <li>
              On the machine running Lucidos, open <strong>Settings → Access</strong>.
            </li>
            <li>
              Under <strong>Pair a device</strong>, press{' '}
              <strong>Create a pairing code</strong>.
            </li>
          </ol>
        )}
        {!scanned && onPhoneSource && <p class="pairing-lede">{onPhoneSource}</p>}
        <label class="pairing-label" for="pairing-code">
          Pairing code
        </label>
        <div class="pairing-code-field">
          <input
            id="pairing-code"
            class="pairing-code-entry"
            value={code}
            onInput={(e) => setCode(applyCodeInput(e.target as HTMLInputElement))}
            onFocus={caretToEnd}
            onClick={caretToEnd}
            inputMode="numeric"
            autocomplete="one-time-code"
            autofocus={!scanned}
          />
          <PairingCodeBoxes code={code} />
        </div>
        {(canScan || canPaste) && (
          <div class="pairing-code-actions">
            {canScan && (
              <button class="pairing-secondary" type="button" onClick={() => setScanning(true)}>
                Scan QR
              </button>
            )}
            {canPaste && (
              <button class="pairing-secondary" type="button" onClick={paste}>
                Paste code
              </button>
            )}
          </div>
        )}
        <label class="pairing-label" for="pairing-label">
          Name this device <span class="pairing-optional">(optional)</span>
        </label>
        <input
          id="pairing-label"
          class="pairing-input"
          value={label}
          onInput={(e) => setLabel((e.target as HTMLInputElement).value)}
        />
        {error && <p class="pairing-error">{error}</p>}
        <button
          class="pairing-submit"
          type="submit"
          disabled={busy || code.length < PAIRING_CODE_DIGITS}
        >
          {busy ? 'Pairing…' : 'Pair'}
        </button>
      </form>
    </div>
  );
}
