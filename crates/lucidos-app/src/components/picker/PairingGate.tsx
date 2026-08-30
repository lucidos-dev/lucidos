/**
 * The pairing screen an unpaired device sees.
 *
 * The gateway authenticates every network caller. An unauthenticated
 * navigation is answered with this screen, at the url it asked for, so this is
 * the one surface a brand-new phone can reach. It stands in front of the
 * picker: paired devices never see it.
 *
 * A browser cannot read the machine-local token file, so a browser on the host
 * pairs exactly like a phone does. The code comes from Settings -> Access on a
 * paired device, which is the one place this screen names. Scanning the QR
 * there lands here with the code already filled in
 * (`utils/pairingCodeSeed.ts`).
 *
 * The desktop app is the exception, and pairs itself: see `DesktopPairing`.
 *
 * # A phone browser is sent to install first
 *
 * iOS gives a home-screen app its own storage container, so the credential
 * cookie taken in Safari never reaches it. Pairing the tab therefore enrols the
 * wrong device. `InstallFirst` says so and shows the install steps. The gateway
 * has already put the scanned code in the manifest's `start_url`, so the app
 * pairs itself the first time it opens.
 *
 * Rest, including what an already-installed app does instead:
 * `docs/plans/2026-08-21-the-installed-app-pairs-itself-on-first-launch.md`.
 */

import { useEffect, useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import {
  PAIRING_CODE_LENGTH,
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

/** The camera scanner, and the QR decoder it carries.
 *
 *  Lazy so `jsqr` stays out of the entry chunk. This screen is the first paint
 *  of a cold launch, and the scanner is the path fewest people take. */
const PairingScanner = lazyComponent<{ onCode: (code: string) => void }>(
  () => import('./PairingScanner'),
);

/** The gateway's own surface, behind the reserved sigil namespace. */
const SESSION_URL = '/~/api/v1/auth/session';

type SessionState = 'checking' | 'paired' | 'unpaired' | 'unreachable';

interface SessionBody {
  paired: boolean;
}

/**
 * Take the boot splash off once this screen paints, and not before.
 *
 * The splash is a full-viewport cover, and nothing else lifts it on this path:
 * `WorkspacePicker` owns that call, and the gate renders a pairing screen
 * INSTEAD of the picker.
 *
 * Every screen below calls this for ITSELF, because two of them decide whether
 * to draw anything at all: the Tauri self-pair and a launch code redeeming on
 * sight both render nothing for the first `SPINNER_DELAY_MS`. A parent lifting
 * the cover on their behalf cannot know that, and would swap a covered screen
 * for a blank one.
 */
function useUncoverOnPaint(painting: boolean) {
  useEffect(() => {
    if (painting) dismissBootSplash();
  }, [painting]);
}

/**
 * Render `children` once this device is paired.
 *
 * `unreachable` renders the children too, deliberately. A gateway that is
 * briefly down, or too old to serve this endpoint, must not replace the picker
 * with a form the user cannot complete. Failing open here costs nothing, since
 * every route behind it is still gated server-side.
 */
export function PairingGate({ children }: { children: ComponentChildren }) {
  const [state, setState] = useState<SessionState>('checking');

  useEffect(() => {
    let live = true;
    fetch(SESSION_URL, { credentials: 'same-origin' })
      .then((r) => (r.ok ? (r.json() as Promise<SessionBody>) : null))
      .then((body) => {
        if (!live) return;
        if (!body) setState('unreachable');
        else setState(body.paired ? 'paired' : 'unpaired');
      })
      .catch(() => live && setState('unreachable'));
    return () => {
      live = false;
    };
  }, []);

  // Unpaired paints entirely out of this module, so nothing is in flight for the
  // inline watchdog to guard. Left armed it reloads this screen every 15 seconds,
  // then covers it with a tap-to-retry splash. The scanner's chunk is lazy, but a
  // tap fetches it long after this, and `lazyComponent` owns that failure. The
  // other two states still render the picker chunk, which hands over on its own
  // once it resolves.
  useEffect(() => {
    if (state === 'unpaired') handOverBootOwnership();
  }, [state]);

  if (state === 'checking') return null;
  if (state === 'unpaired') return <DesktopPairing onPaired={() => setState('paired')} />;
  return <>{children}</>;
}

/**
 * The desktop window pairs itself; every other client types a code.
 *
 * Nothing is asked of the user, because nothing useful could be: a process on
 * this machine is already a pairing authority under ADR 0094, so a button here
 * would add ceremony and no safety. Off Tauri this is the typed form and
 * nothing else.
 *
 * A failure is never a dead end. It falls through to the same form, which still
 * takes a typed code.
 */
function DesktopPairing({ onPaired }: { onPaired: () => void }) {
  const [auto, setAuto] = useState<'running' | 'failed'>(isTauri() ? 'running' : 'failed');
  // Why the automatic attempt gave up, carried into the form rather than
  // swallowed. The form is the recovery. A user who never learns the mint
  // failed has no idea why the Mac is suddenly asking for a code.
  const [autoError, setAutoError] = useState<string | null>(null);
  // Held back past the spinner delay, so a pair that lands in a few hundred
  // milliseconds shows nothing at all. That is the good case: the window opens
  // on the workspace, and the user never learns there was a gate.
  const showBusy = useDelayedFlag(auto === 'running');

  // Only the busy card below. Everything after a failed self-pair is somebody
  // else's screen, and uncovers itself.
  useUncoverOnPaint(auto === 'running' && showBusy);

  useEffect(() => {
    if (auto !== 'running') return;
    let live = true;
    pairDesktopWindow()
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
    // Runs once: `auto` only ever moves to `failed`, which leaves this branch.
  }, []);

  if (auto === 'running') {
    if (!showBusy) return null;
    return (
      <div class="pairing-gate">
        <div class="pairing-card">
          <h1 class="pairing-title">Pairing this Mac…</h1>
          <p class="pairing-lede">Lucidos is letting this window in.</p>
        </div>
      </div>
    );
  }
  return <UnpairedScreen onPaired={onPaired} notice={autoError} />;
}

/** Which screen an unpaired client is owed. */
export type PairingScreenBranch = 'install' | 'form';

/**
 * Pure: `install` for a phone browser, `form` for everything else.
 *
 * iOS gives a home-screen app its own storage container, so a credential taken
 * in Safari never reaches it. A phone browser pairing here would therefore
 * enrol the tab and leave the app the user actually opens still locked out.
 * Sending them to install first makes the device that pairs the device they
 * keep.
 *
 * Desktop answers `form` because there is nothing else it could mean: a desktop
 * browser IS the device, and the host's own window pairs itself.
 */
export function pairingScreenBranch(env: {
  mobile: boolean;
  standalone: boolean;
}): PairingScreenBranch {
  return env.mobile && !env.standalone ? 'install' : 'form';
}

/**
 * The unpaired screen: install first on a phone browser, the form everywhere
 * else.
 *
 * The branch is read once, on mount. Both inputs are fixed for the life of a
 * document, and re-reading them mid-screen could only swap the form out from
 * under somebody typing.
 */
function UnpairedScreen({ onPaired, notice }: { onPaired: () => void; notice?: string | null }) {
  const [branch] = useState(() =>
    pairingScreenBranch({ mobile: thisDeviceIsMobile(), standalone: isStandalone() }),
  );
  // The escape. Set by the "Pair this browser instead" link, never by default:
  // installing is the advice, not the rule.
  const [pairHere, setPairHere] = useState(false);
  if (branch === 'install' && !pairHere) {
    return <InstallFirst onPairHere={() => setPairHere(true)} />;
  }
  return <PairingForm onPaired={onPaired} notice={notice} />;
}

/** Which set of install steps a client is shown. */
export type InstallPlatform = 'ios' | 'android' | 'other';

/** Pure: the platform whose install steps to print. */
export function installPlatformOf(device: { ios: boolean; android: boolean }): InstallPlatform {
  if (device.ios) return 'ios';
  if (device.android) return 'android';
  return 'other';
}

/**
 * Pure: how to install Lucidos on this platform, one step per line.
 *
 * Named menu items rather than a description of them, because the user is
 * holding the device and looking for the words. `other` is deliberately vague:
 * a browser we did not recognise has an install control somewhere, and guessing
 * where is worse than saying to look.
 */
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

/**
 * Install before pairing, on a phone browser.
 *
 * Two ways on from here, and which one applies depends on something we cannot
 * see: whether Lucidos is already on the home screen. A fresh install carries
 * the scanned code into its launch URL and pairs itself, so those steps come
 * first and ask for nothing else. An install that already exists cannot be
 * reached that way, so the code goes across on the pasteboard instead.
 */
function InstallFirst({ onPairHere }: { onPairHere: () => void }) {
  const [scanned] = useState(() => takePairingCodeFromUrl());
  const platform = installPlatformOf({ ios: isIOS(), android: isAndroid() });
  const [copied, setCopied] = useState(false);
  const canCopy = clipboardAbilities().copy;

  // This screen has no silent branch: it draws the recipe or nothing at all.
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
      <div class="pairing-card">
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

/** One drawn box per digit of the code. */
export interface CodeSlot {
  /** The digit typed here, or `''` for a box still waiting. */
  digit: string;
  /** The box the next keystroke lands in. Exactly one slot is active. */
  active: boolean;
  /** Draw a caret here: the box is active AND still empty. The last box of a
   *  full code is active while holding a digit, and a caret over a digit reads
   *  as a second glyph. */
  caret: boolean;
}

/**
 * What a keystroke, a paste or an autofill leaves in the field.
 *
 * A code is decimal, so anything else is a typo or the punctuation somebody
 * pasted around the digits. Dropping it beats refusing the paste.
 */
export function digitsOnly(raw: string): string {
  return raw.replace(/\D/g, '').slice(0, PAIRING_CODE_LENGTH);
}

/**
 * Sanitize the field in place and answer the code it now holds.
 *
 * The length cap lives here rather than on a `maxlength` attribute, which the
 * browser applies to the RAW text before anything sees it: a pasted
 * `4711 8899` would arrive already cut to `4711 88`, and the digits after the
 * space would be gone rather than merely un-spaced.
 *
 * Writing the element back matters for the same reason a controlled input
 * usually does not need it. A rejected character leaves the state unchanged,
 * so nothing re-renders, and the element would keep what the render never
 * agreed to.
 */
export function applyCodeInput(el: { value: string }): string {
  const clean = digitsOnly(el.value);
  if (el.value !== clean) el.value = clean;
  return clean;
}

/**
 * Put the real caret where the drawn one is.
 *
 * The text is invisible, so a tap lands the caret at whatever character
 * boundary it hit. The next digit would go in there rather than in the box the
 * user is looking at. Collapsing to the end keeps the two agreeing, and
 * select-all is untouched because it fires neither event.
 */
function caretToEnd(e: Event) {
  const el = e.target as HTMLInputElement;
  el.setSelectionRange(el.value.length, el.value.length);
}

/**
 * The boxes to draw for `code`.
 *
 * Pure, so the whole affordance is testable: how many boxes, which digit is in
 * which, and where the caret sits at every length from empty to full.
 */
export function codeSlots(code: string, length = PAIRING_CODE_LENGTH): CodeSlot[] {
  const cursor = Math.min(code.length, length - 1);
  return Array.from({ length }, (_, i) => ({
    digit: code[i] ?? '',
    active: i === cursor,
    caret: i === cursor && !code[i],
  }));
}

/**
 * The code field: one real input, drawn as one box per digit.
 *
 * The boxes say where the typing goes and count the digits, which a sample
 * code sitting in the field did neither of: `00000000` reads as a value
 * already entered, and on a phone it is the first thing a thumb tries to
 * delete.
 *
 * They are DRAWN over a single input, never eight of them. One field gets
 * paste, select-all, backspace across a boundary and one-time-code autofill
 * from the browser. Eight would need each of those written by hand, on the one
 * screen a user cannot get past when it breaks. The input keeps the label and
 * the focus. It is transparent rather than hidden, so the browser still treats
 * it as the visible field it is.
 */
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

/**
 * Pure: the sentence a home-screen app is owed about where a code comes from.
 *
 * An installed app has its own storage container, so it can lose its credential
 * while the browser on the same phone keeps one. That browser is then a paired
 * device, and a paired device may mint a code. So the phone can recover itself,
 * and this is the only screen in a position to say so.
 */
export function onPhoneCodeSource(standalone: boolean): string | null {
  if (!standalone) return null;
  return (
    'No other machine to hand? Open the same address in your phone browser. ' +
    'If it is still paired, Settings → Access mints a code you can copy and paste here.'
  );
}

function PairingForm({ onPaired, notice }: { onPaired: () => void; notice?: string | null }) {
  // A launch code seeds this, and only one this client has not already spent.
  // The read is memoized, and the parameter is off the address bar by now. So a
  // re-render never re-seeds a field the user edited.
  const [scanned] = useState(() => takeUnspentPairingCodeFromUrl());
  // Read once: an installed app cannot stop being one mid-screen.
  const [onPhoneSource] = useState(() => onPhoneCodeSource(isStandalone()));
  const [code, setCode] = useState(scanned ?? '');
  // Suggested, not demanded: the browser knows what it is, so offer that and
  // let the user overwrite it. Empty when the browser is unrecognised, which
  // leaves the gateway's own naming in charge.
  const [label, setLabel] = useState(() => suggestDeviceLabelHere() ?? '');
  // Seeded with why the desktop window could not pair itself, when that is why
  // this form is on screen. A submit overwrites it with its own result.
  const [error, setError] = useState<string | null>(notice ?? null);
  const [busy, setBusy] = useState(false);
  // Read once: an ability cannot appear mid-screen, and re-reading it would
  // only make the control flicker.
  const [canPaste] = useState(() => clipboardAbilities().paste);
  // Mobile as well as camera-capable. A laptop has a camera and no QR in front
  // of it, and the machine running Lucidos pairs its own window.
  const [canScan] = useState(() => thisDeviceIsMobile() && cameraIsAvailable());
  const [scanning, setScanning] = useState(false);
  // A launch code is spent on mount, so this screen has nothing to ask for
  // until it comes back refused. `done` covers both "there was no code" and
  // "the code was refused", which are the two states with a form to show.
  const [autoPair, setAutoPair] = useState<'running' | 'done'>(scanned ? 'running' : 'done');
  // Held past the spinner delay, so the ordinary pair shows nothing at all. The
  // boot splash stays up and the reload happens under it, rather than the user
  // reading "Pair this device" about a device already pairing.
  const showAutoPair = useDelayedFlag(autoPair === 'running');
  useUncoverOnPaint(autoPair === 'done' || showAutoPair);

  // Copied on the browser's own pairing screen, pasted here. On iOS this is the
  // one channel that crosses from Safari into the home-screen app, since the
  // two hold separate storage.
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
    setCode(pasted);
  };

  /** Answers whether the code was accepted, so a caller can uncover the form. */
  const redeem = async (value: string): Promise<boolean> => {
    if (busy) return false;
    setBusy(true);
    setError(null);
    try {
      await redeemPairingCode(value, label.trim() || undefined);
      // The credential arrived as an HttpOnly cookie, so there is nothing to
      // store here. Reload so every already-issued request retries with it.
      onPaired();
      window.location.reload();
      return true;
    } catch (e) {
      setError(e instanceof Error ? e.message : 'That code was not accepted.');
      return false;
    } finally {
      setBusy(false);
    }
  };

  const submit = (e: Event) => {
    e.preventDefault();
    void redeem(code);
  };

  // A code in the LAUNCH URL is a decision already taken. It got there because
  // somebody scanned the QR, and on iOS because they then installed this app
  // from the page that carried it. Redeeming it on sight is what makes "it
  // pairs itself when you open it" true rather than a prefilled form.
  //
  // The failure case is why this matters most. A code lives five minutes, and
  // an install can outrun that. Redeeming at once says so immediately, with
  // Scan and Paste already on screen, instead of after a tap on Pair.
  //
  // A pasted or scanned code is NOT auto-submitted: each already cost a
  // deliberate tap, and the user is looking at the field.
  //
  // Only a REFUSAL uncovers the form. A success reloads, and the boot splash
  // rides that navigation into the next document. So the app opens on the
  // picker having asked for nothing.
  useEffect(() => {
    if (scanned) {
      void redeem(scanned).then((paired) => {
        if (!paired) setAutoPair('done');
      });
    }
    // Once, on mount. `scanned` is memoized for the page load, so a re-run
    // could only re-redeem a code this already spent.
  }, []);

  // Nothing to ask for yet, and nothing worth drawing under the delay. The card
  // appears only when the redeem is slow enough that a held splash would read
  // as a stall.
  if (autoPair === 'running') {
    if (!showAutoPair) return null;
    return (
      <div class="pairing-gate">
        <div class="pairing-card">
          <h1 class="pairing-title">Pairing this device…</h1>
          <p class="pairing-lede">Lucidos is letting this app in, with the code it launched with.</p>
        </div>
      </div>
    );
  }

  // The scanner replaces the card's contents rather than floating over them.
  // `<Overlay>` is not reachable here: `OverlayLayer` mounts inside `App`, and
  // this screen renders instead of it. Cancel lives out here, so the card is
  // never a blank box while the scanner's chunk is still loading.
  if (scanning) {
    return (
      <div class="pairing-gate">
        <div class="pairing-card">
          <h1 class="pairing-title">Scan the QR</h1>
          <p class="pairing-lede">
            Point the camera at the QR under Settings → Access on the machine running Lucidos.
          </p>
          <PairingScanner
            onCode={(scannedCode) => {
              setCode(scannedCode);
              setError(null);
              setScanning(false);
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
      <form class="pairing-card" onSubmit={submit}>
        <h1 class="pairing-title">Pair this device</h1>
        <p class="pairing-lede">
          {scanned
            ? 'Lucidos only answers devices you have paired. The code from the QR came in with this launch. If it is refused, it has expired: make a fresh one under Add a device on the other machine, then scan again below.'
            : 'Lucidos only answers devices you have paired. Here is where to get a code.'}
        </p>
        {/* Named menu items, in the order they are tapped, for the same reason
            `installSteps` names them: the user is on the other machine looking
            for these words. */}
        {!scanned && (
          <ol class="pairing-steps">
            <li>
              On the machine running Lucidos, open <strong>Settings → Access</strong>.
            </li>
            <li>
              Under <strong>Add a device</strong>, press{' '}
              <strong>Create a pairing code</strong>.
            </li>
          </ol>
        )}
        {!scanned && onPhoneSource && <p class="pairing-lede">{onPhoneSource}</p>}
        <label class="pairing-label" for="pairing-code">
          Pairing code
        </label>
        {/* Autofocus only when there is something to type. A scanned code
            arrives complete, and focusing it there opens a phone keyboard over
            a field nobody needs to touch. */}
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
        {/* Held until every box is filled. A short code is refused by the
            gateway, and burning the round trip to say so teaches nothing the
            empty boxes do not already show. */}
        <button
          class="pairing-submit"
          type="submit"
          disabled={busy || code.length < PAIRING_CODE_LENGTH}
        >
          {busy ? 'Pairing…' : 'Pair'}
        </button>
      </form>
    </div>
  );
}
