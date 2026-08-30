/**
 * Adopt a pairing code handed over in the URL, so a scanned QR does not make
 * anyone type eight digits.
 *
 * Settings -> Access mints a code and renders a QR for
 * `<reachable-origin>/~/?pair=<code>`. The phone's camera opens that, the
 * picker loads, and `PairingGate` finds the code already in the box.
 *
 * **Stripping the parameter matters for more than tidiness.** A pairing code
 * works once and expires in five minutes, so a URL still carrying it is a URL
 * that will stop working. Left in place it reaches a reload, a bookmark, a
 * shared link and every screenshot. Worse, an iOS PWA installed from that page
 * would keep the dead code in its start URL forever. Same reasoning as
 * `deviceIdSeed.ts`, which established the pattern.
 *
 * Reading is memoized rather than repeated, so ordering cannot matter.
 * `main.tsx` calls this for the strip before anything renders, and
 * `PairingGate` calls it for the value. Whichever runs first does the work.
 */

/** URL parameter carrying the pairing code. Its counterpart is `PAIR_PARAM` in
 *  `crates/lucidos-gateway/src/pairing_qr.rs`, which writes it. */
export const PAIR_CODE_PARAM = 'pair';

/** How many decimal digits the gateway mints (`PAIRING_CODE_DIGITS` in
 *  `crates/lucidos-gateway/src/auth.rs`). The pairing form draws one box per
 *  digit from this, so both readings of "how long is a code" come from here. */
export const PAIRING_CODE_LENGTH = 8;

/** Exactly what the gateway mints. Anything else is dropped rather than sent,
 *  since the only thing a malformed code can do is fail. */
const CODE_RE = new RegExp(`^\\d{${PAIRING_CODE_LENGTH}}$`);

/** Is this exactly a minted code? The one grammar, so a code arriving from the
 *  URL, a paste or a camera is judged by the same rule. */
export function isPairingCode(value: string): boolean {
  return CODE_RE.test(value);
}

/**
 * The code to adopt from a query string, or `null` for "there is none".
 * Pure, so every rejection is testable without a DOM.
 */
export function pairingCodeToAdopt(search: string): string | null {
  let raw: string | null;
  try {
    raw = new URLSearchParams(search).get(PAIR_CODE_PARAM);
  } catch {
    return null;
  }
  const code = raw?.trim() ?? '';
  return isPairingCode(code) ? code : null;
}

/** The result of the one read, or `undefined` before it happened. */
let taken: string | null | undefined;

/**
 * Read the code and drop the parameter from the address bar.
 *
 * Runs once per page load. Later calls hand back the same answer, so a
 * remount cannot read a URL the first call already cleaned.
 *
 * The parameter is stripped whether or not the code is valid, and whether or
 * not this device turns out to be paired already. A stale code in the address
 * bar of a paired device is no more use than one on an unpaired device.
 */
export function takePairingCodeFromUrl(): string | null {
  if (taken !== undefined) return taken;
  if (typeof window === 'undefined') {
    taken = null;
    return taken;
  }
  taken = pairingCodeToAdopt(window.location.search);
  const url = new URL(window.location.href);
  if (url.searchParams.has(PAIR_CODE_PARAM)) {
    url.searchParams.delete(PAIR_CODE_PARAM);
    window.history.replaceState(null, '', url.toString());
  }
  return taken;
}

/**
 * Where a client remembers the launch codes it has already tried.
 *
 * Per storage container, which is the point: a home-screen app and the browser
 * on the same phone hold separate ones, and each spends its own code.
 */
const SPENT_KEY = 'lucidos.pairing.spentLaunchCodes';

/** How many to remember. A launch URL carries one, so this is only slack. */
const SPENT_LIMIT = 8;

function spentLaunchCodes(): string[] {
  try {
    const raw = window.localStorage.getItem(SPENT_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed) ? parsed.filter((c): c is string => typeof c === 'string') : [];
  } catch {
    // No storage, or something else wrote the key. Either way this device has
    // no record, which reads as "not spent" and costs one redeem attempt.
    return [];
  }
}

function rememberSpentLaunchCode(code: string): void {
  try {
    const next = [code, ...spentLaunchCodes().filter((c) => c !== code)].slice(0, SPENT_LIMIT);
    window.localStorage.setItem(SPENT_KEY, JSON.stringify(next));
  } catch {
    // Storage refused. The cost is one dead redeem per launch, which is what
    // this exists to avoid, not something worth a toast on the pairing screen.
  }
}

/**
 * The launch code to REDEEM, which is one this client has not tried before.
 *
 * A code works once, and `takePairingCodeFromUrl` cannot make the launch URL
 * forget it: the strip rewrites the address bar, while iOS relaunches from the
 * `start_url` it stored at install. So an installed icon carries the same code
 * for good, and redeeming on sight would fail on every cold launch.
 *
 * That failure is not free. Each one spends part of the gateway's wrong-guess
 * budget, and a run of launches leaves it refusing even a correct code.
 *
 * Marked spent when it is handed out rather than when the attempt is answered.
 * A code lives five minutes, so one lost to a blip is dead by the next launch,
 * and the form still takes a fresh one.
 *
 * Memoized for the page load, like the read it wraps, and for a sharper reason.
 * Handing the code out is what spends it, so an unmemoized second call would
 * answer `null` to the same document. A remount of the pairing form would then
 * lose the code the first mount was still redeeming.
 */
export function takeUnspentPairingCodeFromUrl(): string | null {
  if (unspent !== undefined) return unspent;
  const code = takePairingCodeFromUrl();
  unspent = !code || spentLaunchCodes().includes(code) ? null : code;
  if (unspent) rememberSpentLaunchCode(unspent);
  return unspent;
}

/** The result of the one unspent read, or `undefined` before it happened. */
let unspent: string | null | undefined;

/** Testing seam: forget both memoized reads. Never called by the app. */
export function resetPairingCodeSeedForTest(): void {
  taken = undefined;
  unspent = undefined;
}
