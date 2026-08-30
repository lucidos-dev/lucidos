import { preferences, showToast, removeToast, notificationsFilter, currentModel, reasoningEffort, selectedCodingAgent, clampThreadDrawerWidth } from '../store';
import type { CodingAgent } from '../../api/types';
import { failedIfFresh } from '../types';
import { getPreferences, setPreference, isTransientFetchError, retryTransientRead } from '../../api/client';
import { getDeviceId } from './devices';
import { errorDetail } from '../../utils/errorDetail';
import { createFailureCounter } from '../../utils/failureCounter';
import { REASONING_LEVELS, DEFAULT_CHAT_MODEL } from '../models';
import { clampEffortFor } from './models';
import { isIOSPwa, isTauri } from '../../utils/platform';
import { publishScrollbarGutter } from '../../utils/scrollbarGutter';
import { setTitlebarColor, windowReadyToShow } from '../../utils/tauri';
import { pushTrafficLightOffset } from './trafficLights';
import {
  STYLE_OVERRIDES_KEY, STYLE_OVERRIDES_STORAGE_KEY, STYLE_RESET_PARAM,
  isValidOverrideName, isValidOverrideValue, parseStyleOverrides,
  serializeStyleOverrides, styleResetRequested,
} from '../../utils/styleOverrides';

import {
  DEFAULT_FONT_FAMILY, DEFAULT_THEME, FONT_FAMILY_VALUES, GOOGLE_FONT_URLS,
  SYSTEM_THEME_SETTLE_MS, THEMES, THEME_BG,
  UI_SCALE_DEFAULT, clampUiScale, fontFeaturesFor, parseUiScale, resolveTheme,
  type FontFamily, type ThemePref,
} from '@lucidos/appearance';

/** Re-exported so the components that already import these from the store keep
 *  one import site. The definitions live in the appearance contract, which is
 *  the single source the two FOUC scripts and the SDK read as well. */
export type { FontFamily } from '@lucidos/appearance';
export type Theme = ThemePref;
export {
  UI_SCALE_MIN, UI_SCALE_MAX, UI_SCALE_STEP, UI_SCALE_DEFAULT, clampUiScale,
} from '@lucidos/appearance';

export type ImageModel = 'auto' | 'imagen-4' | 'gpt-image-1' | 'gpt-image-1.5' | 'gpt-image-2';

// The ligature pair, the font stacks and the two defaults live in
// `@lucidos/appearance` (`packages/lucidos-sdk/src/appearance.ts`), which is the
// single source every surface reads: this store, the two FOUC scripts, and the
// SDK. Its comments carry the reasoning, including the two counter-intuitive
// facts that make a careless edit here a silent no-op (`normal` does NOT mean
// "ligatures off", and a <textarea> does not inherit `font-feature-settings`).
//
// What stays here is the half that is genuinely this module's: reading the
// preference signal, writing the properties onto <html>, and re-asserting the
// style-remote overrides afterwards.

const loadedFonts = new Set<string>();

let systemThemeQuery: MediaQueryList | null = null;
let systemThemeSettleTimer: number | null = null;
// Seeded so loadPreferences can skip a no-op applyTheme when unchanged. The
// matching module-init install of the OS listener sits beside
// `syncSystemThemeListener`, which cannot run before its own constants exist.
let lastAppliedTheme: Theme = currentTheme();

// --- Generic helpers ---

function currentPreference<T extends string>(
  key: string,
  validValues: readonly T[],
  defaultValue: T,
  localStorageKey?: string,
): T {
  if (preferences.value.status === 'loaded') {
    const raw = preferences.value.data[key];
    if (raw && (validValues as readonly string[]).includes(raw)) return raw as T;
  }
  if (localStorageKey) {
    const cached = localStorage.getItem(localStorageKey);
    if (cached && (validValues as readonly string[]).includes(cached)) return cached as T;
  }
  return defaultValue;
}

// --- Preference writes: apply locally, deliver durably ---
//
// `savePreference` applies the value BEFORE the network call (the side effect
// plus the optimistic signal patch), so the UI is never gated on the round trip.
// That makes delivery the only thing that can still go wrong, and on an
// installed iOS PWA it goes wrong constantly for a reason that says nothing
// about the request: WebKit suspends the page (tens of times a day on a busy
// workspace) and aborts every in-flight fetch. The old code toasted that as
// "Failed to save <key> preference: request cancelled", never retried, and left
// the device showing a value the server never received.

/** A preference write the engine has not accepted yet. Parked once an immediate
 *  re-send has also failed transiently, and flushed on the next page resume.
 *
 *  `seq` is the write's position in the global request order, used to spot one
 *  that a newer value for the same key has superseded. `PUT /preferences?key=<k>`
 *  is a per-key overwrite, so this map is last-write-wins rather than a queue: a
 *  value for a key the user has since changed again is garbage, not backlog.
 *  See `docs/glossary.md` § pending preference write. */
interface PendingPreferenceWrite {
  value: string;
  deviceScoped: boolean;
  seq: number;
}

const pendingPreferenceWrites = new Map<string, PendingPreferenceWrite>();
let writeSeq = 0;

/** The newest `seq` requested per key, in-flight ones included. */
const latestWriteSeq = new Map<string, number>();

/** The tail of each key's delivery chain. Two writes to the same key must never
 *  be in flight at once: the engine applies them in ARRIVAL order, so an older
 *  request landing second silently overwrites the user's latest choice, and the
 *  local `seq` bookkeeping cannot see that happen. Serializing also gives the
 *  supersede check below its meaning, because a queued write re-decides whether
 *  it is still wanted at the moment it would actually go out.
 *
 *  Bounded by the number of distinct preference keys, and each entry is dropped
 *  as soon as its chain goes idle. */
const deliveryChains = new Map<string, Promise<void>>();

function enqueueDelivery(key: string, run: () => Promise<void>): Promise<void> {
  const prior = deliveryChains.get(key) ?? Promise.resolve();
  // `then(run, run)` so one rejected link cannot wedge the key's chain forever.
  const next = prior.then(run, run);
  deliveryChains.set(key, next);
  void next.finally(() => {
    if (deliveryChains.get(key) === next) deliveryChains.delete(key);
  });
  return next;
}

/** The engine REJECTED the write (4xx/5xx, a bad body). A verdict the user is
 *  owed, and one no retry can change. Keyed so repeats collapse into one card. */
const PREFERENCE_REJECTED_TOAST = 'preference-save-rejected';
/** The write never GOT to the engine, repeatedly. Keyed separately from the
 *  verdict above so draining the queue can retract this one without also
 *  clearing a rejection the user still needs to read. */
const PREFERENCE_UNREACHABLE_TOAST = 'preference-save-unreachable';

/** Consecutive writes that got no ANSWER. Silent below the threshold, because
 *  the value is applied locally and a re-send is owed, so one suspended fetch is
 *  noise rather than news. At the threshold it speaks once: a genuinely
 *  unreachable engine must not be swallowed. Reset by any answer, a rejection
 *  included, since a 4xx proves the engine is reachable. */
const writeFailures = createFailureCounter(3, () => {
  const stuck = [...pendingPreferenceWrites.keys()].sort().join(', ');
  showToast(
    `Preference changes are not reaching the engine (${stuck}). They are applied on this device and will be re-sent automatically.`,
    'error',
    { key: PREFERENCE_UNREACHABLE_TOAST },
  );
});

/** The engine answered about this key, so it is no longer owed a re-send. Both
 *  answers land here, accepted and refused alike, because both prove the engine
 *  is reachable: the unreachable banner has to be retracted whichever way the
 *  queue drained, or it keeps insisting nothing is getting through while the
 *  rejection card next to it says otherwise. */
function settleDelivered(key: string): void {
  pendingPreferenceWrites.delete(key);
  writeFailures.recordSuccess();
  if (pendingPreferenceWrites.size === 0) removeToast(PREFERENCE_UNREACHABLE_TOAST);
}

/** Queue one preference write behind any other delivery for the same key. Every
 *  path that talks to the engine about a preference goes through here, so writes
 *  to one key are strictly ordered while different keys still go in parallel. */
function deliverOrPark(key: string, write: PendingPreferenceWrite): Promise<void> {
  return enqueueDelivery(key, () => deliverNow(key, write));
}

/** Send one preference write: retry once on a transient rejection, park it for
 *  the next resume if that fails too, surface a real verdict immediately.
 *
 *  Two attempts rather than one because the two transient causes need different
 *  medicine. A radio handoff or a stale connection heals within milliseconds, so
 *  the immediate re-send usually lands (this is `retryTransientRead`'s bargain,
 *  applied to the most idempotent write in the app). A suspended page does not,
 *  so the second failure hands off to the resume flush instead of burning more
 *  attempts against a webview that is not running.
 *
 *  Runs inside the key's delivery chain, so no other write for this key is in
 *  flight and the map bookkeeping below needs no ordering guards of its own. */
async function deliverNow(key: string, write: PendingPreferenceWrite): Promise<void> {
  for (let attempt = 0; attempt < 2; attempt++) {
    // Re-checked per attempt, not once up front: the user can change the same
    // setting again while we wait our turn OR between our two attempts. The
    // newer value owns the key from that moment, and sending ours would put the
    // stale one on the server after it.
    if ((latestWriteSeq.get(key) ?? write.seq) > write.seq) return;
    try {
      await setPreference(key, write.value, write.deviceScoped ? getDeviceId() : undefined);
    } catch (e) {
      if (isTransientFetchError(e)) continue;
      // The engine ANSWERED and refused. No retry can change that, and the user
      // is owed the reason. It also proves the engine is reachable, so this key
      // stops being owed a re-send and the unreachable count resets.
      settleDelivered(key);
      showToast(`Failed to save ${key} preference: ${errorDetail(e)}`, 'error', {
        key: PREFERENCE_REJECTED_TOAST,
      });
      return;
    }
    // Kept OUT of the try: a throw from the bookkeeping below is not a failed
    // save, and catching it here would report one.
    settleDelivered(key);
    return;
  }
  // Both attempts were cancelled, timed out, or dropped in transit. Park the
  // value and stay quiet: the user sees their change applied, and the resume
  // flush delivers it.
  pendingPreferenceWrites.set(key, write);
  writeFailures.recordFailure();
}

/** Re-send every parked preference write. Called from `useStartup`'s resume
 *  handler, which is the moment a suspended iOS PWA can reach the engine again.
 *  A write that fails transiently here stays parked for the next resume. */
export async function flushPendingPreferenceWrites(): Promise<void> {
  if (pendingPreferenceWrites.size === 0) return;
  // Snapshot first: `deliverNow` mutates the map as each write settles.
  const entries = [...pendingPreferenceWrites.entries()];
  await Promise.all(entries.map(([key, write]) => deliverOrPark(key, write)));
}

/** Test-only: the keys whose latest value the engine has not accepted. */
export function _pendingPreferenceKeysForTesting(): string[] {
  return [...pendingPreferenceWrites.keys()].sort();
}

/** Test-only: drop all parked writes, delivery ordering and the escalation
 *  counter, so one case's undelivered write can't leak into the next. */
export function _resetPendingPreferenceWritesForTesting(): void {
  pendingPreferenceWrites.clear();
  latestWriteSeq.clear();
  deliveryChains.clear();
  writeFailures.recordSuccess();
}

export async function savePreference(
  key: string,
  value: string,
  applySideEffect?: () => void,
  deviceScoped = false,
): Promise<void> {
  applySideEffect?.();
  if (preferences.value.status === 'loaded') {
    preferences.value = {
      status: 'loaded',
      data: { ...preferences.value.data, [key]: value },
    };
  }
  const seq = ++writeSeq;
  // Claim the key BEFORE queueing, so any write already in flight or waiting its
  // turn can see it has been superseded and stand down.
  latestWriteSeq.set(key, seq);
  await deliverOrPark(key, { value, deviceScoped, seq });
}

// --- UI scale ---

export function applyUiScale(scale: number): void {
  const clamped = clampUiScale(scale);
  localStorage.setItem('lucidos-ui-scale', String(clamped));
  document.documentElement.style.setProperty('--user-ui-scale', `${clamped}%`);
  // The scrollbar gutter is published in px but our ::-webkit-scrollbar width is
  // authored in rem, so it changes with the root font size this line just moved.
  // Re-measure, or the composer stops lining up with the transcript at any scale
  // other than the one that was live at boot.
  publishScrollbarGutter();
  // Same reason, other quantity: the thread drawer's floor is what its header
  // row needs, and that row is rem-authored too, so scaling up can leave a
  // settled drawer narrower than its own header.
  clampThreadDrawerWidth();
  // This just wrote --user-ui-scale inline, which the remote may be overriding.
  reapplyStyleOverrides();
  // And the same reason a third time, this one outside the page: the macOS
  // traffic lights are centred on the header bar, whose height this line just
  // changed, and only we can tell the shell what it now is.
  //
  // AFTER the re-assert, and that is load-bearing: it MEASURES the rendered
  // header rather than reading the value written above, so with an active
  // --user-ui-scale override it would otherwise measure the preference scale a
  // moment before the override put the real one back, and centre the lights for
  // a bar that never paints. The two measurements above run before the re-assert
  // deliberately, so the gutter they publish is the one actually reserved.
  pushTrafficLightOffset();
}

export function currentUiScale(): number {
  if (preferences.value.status !== 'loaded') return UI_SCALE_DEFAULT;
  const raw = preferences.value.data['ui-scale'] || preferences.value.data['text-size'] || preferences.value.data['font-size'];
  // `parseUiScale` answers null for nothing usable, which for a SETTING means
  // the default. The FOUC scripts want that null instead, so they can leave
  // --user-ui-scale unset and let the stylesheet's own fallback answer.
  return parseUiScale(raw) ?? UI_SCALE_DEFAULT;
}

export function setUiScale(scale: number): Promise<void> {
  const clamped = clampUiScale(scale);
  return savePreference('ui-scale', String(clamped), () => applyUiScale(clamped), true);
}

// --- Theme ---

/** Whether the OS is asking for light right now. The one read point, so a
 *  breadcrumb can never record a different sample than the one that painted. */
function osPrefersLight(): boolean {
  return window.matchMedia('(prefers-color-scheme: light)').matches;
}

export function applyTheme(theme: Theme): void {
  const prefersLight = osPrefersLight();
  const resolved = resolveTheme(theme, prefersLight);
  const bg = THEME_BG[resolved];
  // Theme-flash telemetry — index.html installs __themeLogEvt as a fetch shim
  // that POSTs to /api/v1/internal/client-log (engine.log breadcrumbs).
  type ThemeLogEvt = (label: string, info: unknown) => void;
  const logEvt = (window as unknown as { __themeLogEvt?: ThemeLogEvt }).__themeLogEvt;
  if (logEvt) {
    logEvt('applyTheme', {
      input: theme,
      resolved,
      priorDataTheme: document.documentElement.getAttribute('data-theme'),
      mqLight: prefersLight,
    });
  }
  localStorage.setItem('lucidos-theme', theme);
  document.documentElement.setAttribute('data-theme', resolved);
  document.documentElement.style.setProperty('--bg-primary', bg);
  // Mirrors the inline FOUC IIFE in index.html — keeps <html> covered on
  // toggle and on the next cold reload, before global.css re-applies its
  // `html { background: var(--bg-primary); }` rule.
  document.documentElement.style.background = bg;

  const meta = document.querySelector('meta[name="theme-color"]');
  if (meta) {
    meta.setAttribute('content', bg);
  }

  document.documentElement.style.colorScheme = resolved;

  // Tauri (packaged macOS): match the reclaimed title-bar band's behind-the-
  // webview fallback (the window background) to the in-app header — the
  // header-gradient top stop per theme (mirrors --header-gradient in
  // styles/global/base.css, like the --bg-primary literal above; the visible
  // band itself is the CSS .titlebar-strip). Best-effort and cosmetic: it runs
  // whenever the theme is applied (incl. startup / system-theme changes) with no
  // user-facing surface, and a failed call self-heals on the next applyTheme, so
  // a toast would be wrong.
  if (isTauri()) {
    const titlebar = resolved === 'light' ? '#1a6fd0' : '#15549e';
    setTitlebarColor(titlebar).catch((e) => console.warn('[titlebar] tint failed', e));
    // The theme is resolved and on the document, so a window shown now shows a
    // page in the user's theme. The shell keeps the launch window hidden until
    // it hears this. One-shot inside the wrapper, since this line also runs on
    // every toggle and system-appearance change.
    windowReadyToShow();
  }

  syncSystemThemeListener(theme);
  lastAppliedTheme = theme;
  // This just wrote --bg-primary inline and swapped the token block wholesale,
  // so any override of a themed token has to be re-asserted on top.
  reapplyStyleOverrides();
}

// --- Following the OS under a `system` preference ---
//
// Two things go wrong if `system` is resolved only from the media query's
// `change` event, and the guards below answer one each.
//
// Backgrounding an iOS app makes UIKit flip its trait collection to the
// opposite appearance and straight back, to render both app-switcher snapshots
// (rdar://7213631). WKWebView passes each flip into the page as a real media
// query change. Acting on one paints an appearance that existed for the
// snapshot alone. That is the light flash telemetry caught 24+ times in one
// session, and it is why this listener was once skipped on iOS entirely.
//
// The event can also simply never arrive. An installed iOS PWA is resumed
// rather than reloaded. A frozen desktop tab runs no JavaScript, and a sleeping
// machine wakes into an appearance nobody announced.

/** The three events one iOS wake delivers together, per
 *  `docs/plans/2026-08-03-ios-pwa-resume-storm-and-durable-compose-drafts.md`.
 *  Each schedules the same settle timer, so a wake costs one read.
 *
 *  Deliberately NOT `onPageWake` (`utils/pageVisit.ts`), which fires only when
 *  a hide preceded. A window that merely lost focus never went hidden, so a Mac
 *  that slept through the flip would get nothing back. That is one of the two
 *  cases this exists for. */
const SYSTEM_THEME_RESUME_EVENTS: ReadonlyArray<readonly [EventTarget, string]> = [
  [document, 'visibilitychange'],
  [window, 'focus'],
  [window, 'pageshow'],
];

/** Re-resolve `system` and apply it, if all three guards pass: the preference
 *  still follows the OS, the user is actually looking at the page, and the
 *  resolved value is not the one already painted.
 *
 *  The visibility guard is what makes the media-query listener safe on iOS: a
 *  snapshot-pass flip arrives while the app is backgrounded, so it is dropped
 *  rather than painted. */
function refreshSystemTheme(): void {
  if (currentTheme() !== 'system') return;
  if (document.visibilityState !== 'visible') return;
  const resolved = resolveTheme('system', osPrefersLight());
  if (document.documentElement.getAttribute('data-theme') === resolved) return;
  applyTheme('system');
}

/** Arm one shared settle timer, which re-READS the OS when it fires. Nothing
 *  ever applies the value an event carried: a flip that raced the visibility
 *  guard has been corrected by the time this samples.
 *
 *  An already-armed timer is left alone rather than pushed back. A burst then
 *  resolves one settle delay after its first event, not after its last. */
function scheduleSystemThemeRefresh(): void {
  if (systemThemeSettleTimer !== null) return;
  systemThemeSettleTimer = window.setTimeout(() => {
    systemThemeSettleTimer = null;
    refreshSystemTheme();
  }, SYSTEM_THEME_SETTLE_MS);
}

/** Subscribe to the OS appearance while the preference is `system`, and to
 *  nothing at all otherwise. Called from every `applyTheme`, so it tears the
 *  previous registration down first and is safe to run repeatedly. */
function syncSystemThemeListener(theme: Theme): void {
  systemThemeQuery?.removeEventListener('change', scheduleSystemThemeRefresh);
  systemThemeQuery = null;
  for (const [target, type] of SYSTEM_THEME_RESUME_EVENTS) {
    target.removeEventListener(type, scheduleSystemThemeRefresh);
  }
  if (systemThemeSettleTimer !== null) {
    clearTimeout(systemThemeSettleTimer);
    systemThemeSettleTimer = null;
  }
  if (theme !== 'system') return;

  systemThemeQuery = window.matchMedia('(prefers-color-scheme: light)');
  systemThemeQuery.addEventListener('change', scheduleSystemThemeRefresh);
  for (const [target, type] of SYSTEM_THEME_RESUME_EVENTS) {
    target.addEventListener(type, scheduleSystemThemeRefresh);
  }
}

// Module-init install. loadPreferences skips applyTheme when the stored theme
// already matches lastAppliedTheme. Without this call a user on `system` would
// never get the OS listener attached.
syncSystemThemeListener(lastAppliedTheme);

/** The device's theme, defaulting to `system` (follow the OS light/dark
 *  setting). A device that has explicitly picked light or dark keeps its pick:
 *  this is only what applies when nothing is stored, which is also why changing
 *  it reaches existing devices that never opened Settings.
 *
 *  The default is mirrored in the FOUC script (index.html), the iframe FOUC
 *  script (api/sdk_prefs.rs), the SDK (`resolveThemePreference`) and the
 *  preference catalog. They paint at different moments of one page load, so a
 *  disagreement between them is a visible flash. */
export function currentTheme(): Theme {
  // localStorage fallback matches the FOUC prevention script in index.html.
  // Covers: backend missing the preference (device_id change, save failure),
  // and the loading window before the API responds.
  return currentPreference('theme', THEMES, DEFAULT_THEME, 'lucidos-theme');
}

export function setTheme(theme: Theme): Promise<void> {
  return savePreference('theme', theme, () => applyTheme(theme), true);
}

// --- Font family ---

function ensureFontLoaded(font: FontFamily): void {
  const url = GOOGLE_FONT_URLS[font];
  if (!url || loadedFonts.has(font)) return;
  loadedFonts.add(font);
  const link = document.createElement('link');
  link.rel = 'stylesheet';
  link.href = url;
  document.head.appendChild(link);
}

export function applyFontFamily(font: FontFamily): void {
  ensureFontLoaded(font);
  localStorage.setItem('lucidos-font-family', font);
  document.documentElement.style.setProperty('--font-ui', FONT_FAMILY_VALUES[font]);
  const features = fontFeaturesFor(font);
  document.documentElement.style.setProperty('--font-features-text', features.text);
  document.documentElement.style.setProperty('--font-features-code', features.code);
  // This just wrote --font-ui and the two feature properties inline.
  reapplyStyleOverrides();
}

/** The device's UI font. The default and the valid set come from the appearance
 *  contract, so this and the boot scripts cannot disagree.
 *
 *  Deliberately NOT backed by localStorage the way the theme is: `applyFontFamily`
 *  writes `lucidos-font-family` on every load for the FOUC script to read, so a
 *  cached value here would just be the previous default echoed back and would
 *  outlive a change to it. */
export function currentFontFamily(): FontFamily {
  return currentPreference(
    'font-family',
    Object.keys(FONT_FAMILY_VALUES) as FontFamily[],
    DEFAULT_FONT_FAMILY,
  );
}

export function setFontFamily(font: FontFamily): Promise<void> {
  return savePreference('font-family', font, () => applyFontFamily(font), true);
}

// --- Style overrides (the live style remote) ---

/** The custom property names currently written onto `<html>` by this module.
 *  Kept so a name DROPPED from the map is `removeProperty`'d rather than left
 *  stuck at its last value: an inline property outlives the map that set it,
 *  so "clear" would otherwise only take effect on the next reload. */
let appliedOverrideNames: string[] = [];

/** Write the map onto the root element, removing any name that has since left
 *  it. Validation happens here as well as at parse time, because a caller can
 *  hand a map straight in. */
export function applyStyleOverrides(map: Record<string, string>): void {
  const root = document.documentElement;
  // Apply first and record what ACTUALLY landed, then remove anything that was
  // applied before and is not in that set. Keying the removal on the incoming
  // map instead would leave a stale value stuck: a name whose new value fails
  // validation is still `in map`, so it would be skipped by both loops and keep
  // painting its previous value, from a function that promises to validate.
  const applied: string[] = [];
  for (const [name, value] of Object.entries(map)) {
    if (!isValidOverrideName(name) || !isValidOverrideValue(value)) continue;
    root.style.setProperty(name, value);
    applied.push(name);
  }
  for (const name of appliedOverrideNames) {
    if (!applied.includes(name)) root.style.removeProperty(name);
  }
  appliedOverrideNames = applied;
  localStorage.setItem(STYLE_OVERRIDES_STORAGE_KEY, serializeStyleOverrides(map));
  // A retuned --font-size-* or spacing token moves the root font size's
  // consumers, and the scrollbar gutter is measured in px off rem-authored
  // chrome. Same reason applyUiScale re-measures.
  publishScrollbarGutter();
  // Same reason again: --user-ui-scale is itself overridable, and the thread
  // drawer's floor is what its rem-authored header row needs.
  clampThreadDrawerWidth();
  // And so is the bar the macOS traffic lights are centred on: a retuned
  // --user-ui-scale or --desktop-bar-height moves it, and the shell only learns
  // that from here.
  pushTrafficLightOffset();
}

/** Re-assert the overrides after something else has written the same
 *  properties inline. `applyTheme` writes `--bg-primary`, `applyUiScale` writes
 *  `--user-ui-scale`, `applyFontFamily` writes `--font-ui`: each is a property
 *  the remote is allowed to override, and each of those three calls this at the
 *  end so the override keeps winning. Without it a system-theme flip silently
 *  reverts a tuned background. */
export function reapplyStyleOverrides(): void {
  if (appliedOverrideNames.length === 0) return;
  const root = document.documentElement;
  const map = currentStyleOverrides();
  for (const [name, value] of Object.entries(map)) {
    root.style.setProperty(name, value);
  }
}

export function currentStyleOverrides(): Record<string, string> {
  if (preferences.value.status !== 'loaded') {
    return parseStyleOverrides(localStorage.getItem(STYLE_OVERRIDES_STORAGE_KEY));
  }
  return parseStyleOverrides(preferences.value.data[STYLE_OVERRIDES_KEY]);
}

export function clearStyleOverrides(): Promise<void> {
  return savePreference(STYLE_OVERRIDES_KEY, '{}', () => applyStyleOverrides({}), true);
}

/** Apply whatever the loaded preferences say, honouring the `?style-reset`
 *  escape hatch. Called at the END of `loadPreferences`, after theme / scale /
 *  font, so an override of one of their properties wins.
 *
 *  Never throws. This is DECORATION running inside the load of everything the
 *  app needs to function: letting it escape would turn one bad custom property
 *  into a `failed` preferences state, which blanks the user's model, theme,
 *  reasoning effort and coding agent. The carve-out in `.claude/rules/frontend.md`
 *  applies (no user intent on this line, and it self-recovers): every token
 *  simply keeps its stylesheet value, the next `PreferencesChanged` re-runs
 *  this, and a genuinely wrong value has two user-facing routes out, the
 *  Settings row and `?style-reset`. */
/** Whether `?style-reset` has already been honoured in this document.
 *
 *  The reset MUST run at most once per page load. `loadPreferences` re-runs on
 *  every `PreferencesChanged`, and the clear is itself a preference write that
 *  emits one, so a URL still carrying the parameter would drive an endless
 *  write/SSE/reload loop: clear, fan out, reload, see the parameter, clear
 *  again. The parameter is also stripped from the URL below, so a later reload
 *  of the same tab does not silently wipe values tuned since. */
let styleResetHonoured = false;

function applyStyleOverridesFromPreferences(): void {
  try {
    // `window.location` is absent in non-DOM environments, and a missing
    // search string must not be what decides whether preferences load.
    const search = typeof window !== 'undefined' ? (window.location?.search ?? '') : '';
    if (!styleResetHonoured && styleResetRequested(search)) {
      styleResetHonoured = true;
      dropStyleResetParam();
      // Fire-and-forget with a caught rejection: the reset must clear the LOCAL
      // paint immediately even if the engine is unreachable, which is a
      // plausible state for someone who just made their UI unusable.
      void clearStyleOverrides().catch((e) => console.warn('[style-remote] reset write failed', e));
      return;
    }
    applyStyleOverrides(currentStyleOverrides());
  } catch (e) {
    console.warn('[style-remote] applying overrides failed', e);
  }
}

/** Take `?style-reset` out of the address bar once it has been honoured, so a
 *  refresh, a restored tab or a shared link does not keep clearing overrides.
 *  Only the query parameter is touched: the path and the hash carry the app's
 *  own routing. */
function dropStyleResetParam(): void {
  if (typeof window === 'undefined' || !window.history?.replaceState) return;
  const url = new URL(window.location.href);
  url.searchParams.delete(STYLE_RESET_PARAM);
  window.history.replaceState(null, '', `${url.pathname}${url.search}${url.hash}`);
}

// --- Image model ---

export function currentImageModel(): ImageModel {
  return currentPreference('image_model', ['auto', 'imagen-4', 'gpt-image-1', 'gpt-image-1.5', 'gpt-image-2'], 'auto');
}

export function setImageModel(model: ImageModel): Promise<void> {
  return savePreference('image_model', model);
}

// --- Notifications filter ---

export function currentNotificationsFilter(): 'all' | 'unread' {
  return currentPreference('notifications_filter', ['all', 'unread'], 'all', 'lucidos-notifications-filter');
}

// --- In-app notification toasts ---

/** Device-local mirror of the served value, and the reason this preference is
 *  not read straight off the signal like the other booleans here.
 *
 *  A toast is unsolicited and cannot be taken back, so the window before
 *  preferences load is not a harmless default: it hands the interruption to
 *  exactly the user who turned it off. That window is a whole round trip on an
 *  iOS PWA over Tailscale, on an app the OS evicts constantly. A load that
 *  FAILS never closes it at all. The cache answers from the last known value
 *  instead, and the served value wins the moment it lands. */
const NOTIFICATION_TOASTS_KEY = 'lucidos-notification-toasts';

/** Whether a notification may pop a toast over what the user is doing. Only an
 *  explicit `'false'` silences it, so unset behaves as it always has. */
export function currentNotificationToasts(): boolean {
  return currentPreference(
    'notification_toasts', ['true', 'false'], 'true', NOTIFICATION_TOASTS_KEY,
  ) === 'true';
}

/** Take the mirror from what the engine just served. An absent key CLEARS it
 *  rather than leaving it: unset means the default, and a cache kept there
 *  would outlive a reset and keep answering for a preference nobody holds. */
function cacheNotificationToasts(): void {
  const served = preferences.value.status === 'loaded'
    ? preferences.value.data['notification_toasts']
    : undefined;
  if (served === 'true' || served === 'false') {
    localStorage.setItem(NOTIFICATION_TOASTS_KEY, served);
  } else {
    localStorage.removeItem(NOTIFICATION_TOASTS_KEY);
  }
}

export function setNotificationToasts(enabled: boolean): Promise<void> {
  const value = enabled ? 'true' : 'false';
  return savePreference('notification_toasts', value, () => {
    localStorage.setItem(NOTIFICATION_TOASTS_KEY, value);
  });
}

// --- Chat model & reasoning effort ---

const REASONING_VALUES = REASONING_LEVELS.map(l => l.value);

/** The user's chat model preference. Unlike most preferences this is NOT
 *  validated against a fixed allow-list — the model set is now the DB-backed
 *  registry (user-extensible), and `RoutingProvider` resolves any id (with a
 *  prefix fallback), so any stored non-empty value is honored. */
export function currentChatModel(): string {
  if (preferences.value.status !== 'loaded') return DEFAULT_CHAT_MODEL;
  const v = preferences.value.data['chat_model'];
  return v && v.trim() ? v : DEFAULT_CHAT_MODEL;
}

export function currentChatReasoningEffort(): string {
  return currentPreference('chat_reasoning_effort', REASONING_VALUES, 'high');
}

/** Persist the chat *model selection*, both halves.
 *
 *  One pick sets the pair, so the effort is the user's own choice rather than a
 *  clamp of the previous one. A model with no tiers reports `null` and leaves
 *  the stored effort alone, which nothing acts on: the picker clamps for
 *  display and `RoutingProvider::effort_for_model` clamps the request. */
export async function setChatModelSelection(
  patch: { model: string; reasoningEffort: string | null },
): Promise<void> {
  await savePreference('chat_model', patch.model, () => { currentModel.value = patch.model; });
  if (patch.reasoningEffort !== null) await setReasoningEffort(patch.reasoningEffort);
}

export function setReasoningEffort(effort: string): Promise<void> {
  return savePreference('chat_reasoning_effort', effort, () => {
    reasoningEffort.value = effort;
  });
}

// --- Max tool calls (the per-turn tool-call cap) ---

/** Mirrors `DEFAULT_MAX_TOOL_CALLS` in `core/preferences.rs`. */
export const MAX_TOOL_CALLS_DEFAULT = 500;

/** Mirrors `MIN_MAX_TOOL_CALLS` in `core/preferences.rs`. There is deliberately
 *  no maximum: a high cap costs the user time and tokens, which is their call to
 *  make. The floor rules out only the value that is broken rather than small,
 *  since the loop checks `iterations > cap` after incrementing and a cap of 0
 *  would end the turn before the first LLM call. */
export const MAX_TOOL_CALLS_MIN = 1;

/** The largest cap the UI will write. This is NOT the policy ceiling the design
 *  deliberately omits, it is the point where JavaScript stops being able to
 *  carry the number: past `Number.MAX_SAFE_INTEGER`, `Number('…')` rounds (and
 *  eventually reaches `Infinity`), so `String(Number(input))` would save a
 *  *different* value than the user typed and the engine would then enforce, or
 *  reject, something else again. Nine quadrillion tool calls is not a cap anyone
 *  reaches; the bound exists so the UI cannot display a value the engine will
 *  not honor. */
export const MAX_TOOL_CALLS_REPRESENTABLE = Number.MAX_SAFE_INTEGER;

/** Roughly how long a turn can run at a given cap, in seconds per tool call.
 *  The LLM round-trip dominates a step: ~15s for a large-context reasoning
 *  model, faster on a small model with light tools, slower under heavy `bash`
 *  work. Reads as an upper estimate, because a step that batches several tool
 *  calls pays one round-trip for all of them. Used only to show the user what a
 *  number means before they pick it, so an order of magnitude is the point. */
const SECONDS_PER_TOOL_CALL = 15;

/** A human "about N hours" (or minutes/days) for a cap, for the Settings note.
 *  Deliberately coarse: the point is the order of magnitude, not a promise. */
export function estimateTurnDuration(maxToolCalls: number): string {
  const minutes = (maxToolCalls * SECONDS_PER_TOOL_CALL) / 60;
  if (minutes < 60) return `${Math.max(1, Math.round(minutes))} min`;
  const hours = minutes / 60;
  if (hours < 48) return `${hours < 10 ? hours.toFixed(1).replace(/\.0$/, '') : Math.round(hours)} hours`;
  return `${Math.round(hours / 24)} days`;
}

/** The per-turn tool-call cap. Mirrors `PreferenceStore::max_tool_calls` so the
 *  UI never displays a value the engine would not honor: an absent or
 *  unparseable value shows the default, and a parsed value is raised to the
 *  floor. A large value is shown as stored, since there is no ceiling. */
export function currentMaxToolCalls(): number {
  if (preferences.value.status !== 'loaded') return MAX_TOOL_CALLS_DEFAULT;
  const raw = preferences.value.data['max_tool_calls'];
  if (raw == null) return MAX_TOOL_CALLS_DEFAULT;
  // Integers only, matching the engine's `parse::<usize>()`: it rejects "12.5"
  // and "-5" outright, where `parseInt` would happily read 12 and -5.
  const trimmed = raw.trim();
  if (!/^\d+$/.test(trimmed)) return MAX_TOOL_CALLS_DEFAULT;
  // A value JS cannot carry exactly could only have been written outside this
  // UI (the CLI, the HTTP API, psql), since `setMaxToolCalls` refuses to write
  // one. Show the bound rather than a silently rounded number.
  const parsed = Number(trimmed);
  if (parsed > MAX_TOOL_CALLS_REPRESENTABLE) return MAX_TOOL_CALLS_REPRESENTABLE;
  return Math.max(MAX_TOOL_CALLS_MIN, parsed);
}

export function setMaxToolCalls(maxToolCalls: number): Promise<void> {
  return savePreference('max_tool_calls', String(maxToolCalls));
}

// --- Locale (language + timezone) ---
//
// Both are GLOBAL (workspace-wide, not device-scoped). Writing them goes through
// PUT /preferences → the engine's apply_preference_write chokepoint, which
// refreshes the engine's in-memory user_language/user_timezone and emits
// LanguageSet/TimezoneSet — so the change takes effect with no restart, and the
// frontend live-refreshes (thread-sync reloads preferences on those events).

export function currentLanguage(): string {
  if (preferences.value.status !== 'loaded') return '';
  return preferences.value.data['language'] || '';
}

export function setLanguage(language: string): Promise<void> {
  return savePreference('language', language.trim());
}

export function currentTimezone(): string {
  if (preferences.value.status !== 'loaded') return '';
  return preferences.value.data['timezone'] || '';
}

export function setTimezone(timezone: string): Promise<void> {
  return savePreference('timezone', timezone);
}

// --- Load all preferences ---

// Monotonic token per call, same idea as `fetchAttemptSeq` in
// thread-loading.ts. A resume can now call this while an SSE-triggered
// refetch is still pending, so two independent GETs can be in flight.
//
// Sharing one in-flight PROMISE would be the wrong fix here. WebKit can
// leave a fetch hanging across an iOS suspension, with nothing to await
// ever settling. The resume that exists to recover from exactly that
// would then be stuck behind it forever. So every call issues its own
// fetch. Only the newest ISSUED call's outcome is applied. An older one
// landing after a newer one was issued is silently discarded.
let preferencesLoadSeq = 0;

export async function loadPreferences(): Promise<void> {
  const mySeq = ++preferencesLoadSeq;
  // Only flip to 'loading' on the first fetch — refetches (e.g. after an SSE
  // PreferencesChanged) keep showing existing data through the network round
  // trip and swap atomically when the response lands. Without this guard,
  // every preference toggle wipes subscribers to defaults until the GET
  // completes, which the user sees as a flash.
  if (preferences.value.status === 'not-loaded') {
    preferences.value = { status: 'loading' };
  }
  try {
    // Retry a transient rejection before flipping to `failed`, same as
    // `loadRepositories` (repositoriesLoader.ts). Nothing re-triggers this
    // load once it fails (SSE only re-fires an already-`loaded` value): a
    // single cancelled startup fetch would otherwise paint every setting at
    // its default for the rest of the page load.
    const res = await retryTransientRead(() => getPreferences(getDeviceId()));
    // A newer call was issued while this one was in flight: its outcome
    // wins, so applying this stale one would overwrite fresher data.
    if (mySeq !== preferencesLoadSeq) return;
    preferences.value = { status: 'loaded', data: res.preferences };
    applyUiScale(currentUiScale());
    const t = currentTheme();
    if (t !== lastAppliedTheme) applyTheme(t);
    applyFontFamily(currentFontFamily());
    currentModel.value = currentChatModel();
    reasoningEffort.value = clampEffortFor(currentChatReasoningEffort(), currentModel.value);
    notificationsFilter.value = currentNotificationsFilter();
    cacheNotificationToasts();
    selectedCodingAgent.value = currentCodingAgentDefault();
    // LAST, deliberately: the three applies above write properties the remote
    // is allowed to override, so the overrides go on top of them.
    applyStyleOverridesFromPreferences();
  } catch (e) {
    if (mySeq !== preferencesLoadSeq) return;
    // A failed REFETCH keeps the loaded preferences rather than blanking every
    // setting to its default. SSE only re-fires an already-`loaded` value, so a
    // flip to `failed` would never recover until a page reload. Only a first
    // load records the failure. Matches `loadWebhookIngress`.
    preferences.value = failedIfFresh(preferences.value, e);
  }
}

// --- Vertex AI region ---

export const DEFAULT_VERTEX_REGION = 'europe-west1';

export function currentVertexRegion(): string {
  if (preferences.value.status !== 'loaded') return DEFAULT_VERTEX_REGION;
  return preferences.value.data['vertex_region'] || DEFAULT_VERTEX_REGION;
}

export function setVertexRegion(region: string): Promise<void> {
  return savePreference('vertex_region', region);
}

// --- Local OpenAI-compatible provider base URL ---

// Ollama's OpenAI-compatible endpoint. Mirrors DEFAULT_LOCAL_BASE_URL in
// crates/lucidos-engine/src/core/preferences.rs.
export const DEFAULT_LOCAL_BASE_URL = 'http://localhost:11434/v1';

export function currentLocalBaseUrl(): string {
  if (preferences.value.status !== 'loaded') return '';
  return preferences.value.data['local_base_url'] || '';
}

export function setLocalBaseUrl(url: string): Promise<void> {
  return savePreference('local_base_url', url.trim());
}

// --- OpenCode Free (keyless) ---

/** Whether the keyless OpenCode Free tier is on. Off by default: only an
 *  explicit `'true'` opts in, because turning it on sends prompts anonymously
 *  to a third-party relay. */
export function currentOpenCodeFreeEnabled(): boolean {
  if (preferences.value.status !== 'loaded') return false;
  return preferences.value.data['opencode_free_enabled'] === 'true';
}

export function setOpenCodeFreeEnabled(enabled: boolean): Promise<void> {
  return savePreference('opencode_free_enabled', enabled ? 'true' : 'false');
}

// --- Per-provider enable switches ---

/** Providers whose switch is the `provider_enabled_<id>` preference. OpenCode
 *  Free is deliberately absent: it is opt-IN under its own key (ADR 0104),
 *  where these six are opt-OUT. Ids match the engine's `ProviderKind`. */
export type SwitchableProvider =
  | 'vertex' | 'anthropic' | 'openai' | 'openrouter' | 'xai' | 'local';

function providerEnabledKey(id: SwitchableProvider): string {
  return `provider_enabled_${id}`;
}

/** Whether the user has explicitly switched this provider OFF.
 *
 *  Not the inverse of "is it running". Absent means enabled, so this is false
 *  both for a provider left alone and for one that was never configured. What
 *  it distinguishes is "switched off" from "never set up", which is the only
 *  thing the raw preference can tell you that `/health` cannot. */
export function providerSwitchedOff(id: SwitchableProvider): boolean {
  if (preferences.value.status !== 'loaded') return false;
  return preferences.value.data[providerEnabledKey(id)] === 'false';
}

/** Switch a provider on or off. Off leaves the stored credential alone: the
 *  switch is how a user parks a key they still want. */
export function setProviderEnabled(
  id: SwitchableProvider,
  enabled: boolean,
): Promise<void> {
  return savePreference(providerEnabledKey(id), enabled ? 'true' : 'false');
}

// --- Capture context ---

/** Per-step ContextAssembled capture toggle. Defaults to false (off) — the
 *  debugging capture ships dark; only an explicit `'true'` opts in. */
export function currentCaptureContext(): boolean {
  if (preferences.value.status !== 'loaded') return false;
  const raw = preferences.value.data['capture_context'];
  if (raw == null) return false;
  return raw === 'true';
}

export function setCaptureContext(enabled: boolean): Promise<void> {
  return savePreference('capture_context', enabled ? 'true' : 'false');
}

// --- Experimental: in-app browser (Tauri native webview) ---

/** Whether URL previews open in the in-app native webview ("Tauri browser")
 *  instead of the system browser. Experimental and desktop-only — the native
 *  webview exists only under Tauri; web/PWA always opens a new tab. Defaults to
 *  false (off): URLs open in the system browser unless the user opts in. Only an
 *  explicit `'true'` enables the in-app webview. */
export function currentInAppBrowser(): boolean {
  if (preferences.value.status !== 'loaded') return false;
  return preferences.value.data['experimental_in_app_browser'] === 'true';
}

export function setInAppBrowser(enabled: boolean): Promise<void> {
  return savePreference('experimental_in_app_browser', enabled ? 'true' : 'false');
}

/** Whether the in-app browser is the live URL target in this client: the native
 *  webview exists only in the desktop app, AND the experimental toggle has to be
 *  on. The single definition of that pair, so the surfaces that must agree about
 *  it cannot drift: the menu drawer's Browser row (its only entry point),
 *  `restoreState`'s refusal to resurrect a url-preview overlay, and `openUrl`
 *  deciding between the panel and `openUrlOutsideApp`. */
export function inAppBrowserAvailable(): boolean {
  return isTauri() && currentInAppBrowser();
}

// --- External link target (installed iOS PWA only) ---

/** Where an external http(s) link goes when tapped in an INSTALLED iOS PWA.
 *  Consulted only there: every other client (desktop web, Android, a normal
 *  Safari tab, and all three Tauri branches) opens a new tab / the OS opener
 *  regardless. See `utils/openExternalUrl.ts` for what each mode does and why
 *  the platform forces the choice on us at all. */
export type ExternalLinkTarget = 'safari' | 'ask' | 'in-app';

const EXTERNAL_LINK_TARGETS: readonly ExternalLinkTarget[] = ['safari', 'ask', 'in-app'];

export const DEFAULT_EXTERNAL_LINK_TARGET: ExternalLinkTarget = 'safari';

/** Defaults to `'safari'` (the shipped behaviour) both when unset and while
 *  preferences are still loading, so a link tapped during startup can't fall
 *  into a different mode than the same link tapped a second later. An
 *  unrecognized stored value degrades to the default rather than disabling the
 *  hand-off. */
export function currentExternalLinkTarget(): ExternalLinkTarget {
  if (preferences.value.status !== 'loaded') return DEFAULT_EXTERNAL_LINK_TARGET;
  const stored = preferences.value.data['external_link_target'];
  return EXTERNAL_LINK_TARGETS.includes(stored as ExternalLinkTarget)
    ? stored as ExternalLinkTarget
    : DEFAULT_EXTERNAL_LINK_TARGET;
}

export function setExternalLinkTarget(target: ExternalLinkTarget): Promise<void> {
  return savePreference('external_link_target', target);
}

/** Whether this client is one where the target actually decides anything, and
 *  so the Settings row is worth showing. Only an installed iOS PWA is: every
 *  other client opens a new tab (or the desktop OS opener) whatever the stored
 *  value says. Rendering the row elsewhere would be a control that does nothing.
 *
 *  Deliberately NOT read by `openExternalUrl`, which re-checks `isIOSPwa()` on
 *  its own, so the routing cannot come to depend on a Settings-facing helper. */
export function externalLinkTargetConfigurable(): boolean {
  return isIOSPwa();
}

// --- Mobile header sticky ---

/** When true, the mobile header stays fully visible — disables hide-on-scroll,
 *  hide-on-keyboard-open, and the app-UI-active pin. Defaults to true; only an
 *  explicit `'false'` opts out of the pinned header. */
export function currentMobileHeaderSticky(): boolean {
  if (preferences.value.status !== 'loaded') return true;
  return preferences.value.data['mobile_header_sticky'] !== 'false';
}

export function setMobileHeaderSticky(enabled: boolean): Promise<void> {
  return savePreference('mobile_header_sticky', enabled ? 'true' : 'false');
}

// --- Background model ---

/** Background model preference keys, stored in the DB and read by the engine.
 *  Each is half of a *model selection*; the paired `reasoning_*` key below
 *  carries the effort. The engine resolves the pair per `ContextPurpose` in
 *  `engine::aux_purpose`. */
export type BackgroundModelKey =
  | 'model_title'
  | 'model_image_description'
  | 'model_memory'
  | 'model_conversation_summary'
  | 'model_command_judge';

/** The reasoning half of each background *model selection*. */
export type BackgroundReasoningKey =
  | 'reasoning_title'
  | 'reasoning_image_description'
  | 'reasoning_memory'
  | 'reasoning_conversation_summary'
  | 'reasoning_command_judge';

/** Default model for the command-guard judge (Haiku, per ADR 0002). Mirrors the
 *  backend `DEFAULT_COMMAND_JUDGE_MODEL` in `core/preferences.rs`. */
export const DEFAULT_COMMAND_JUDGE_MODEL = 'claude-haiku-4-5';

/** Per-key default shown when the preference is unset. Most background tasks
 *  default to Gemini Flash; the command-guard judge defaults to Haiku. The
 *  conversation summary inherits the memory model until it is set, matching
 *  `aux_purpose`'s `model_fallback_key`. */
const BACKGROUND_MODEL_DEFAULTS: Record<BackgroundModelKey, string> = {
  model_title: 'gemini-3-flash-preview',
  model_image_description: 'gemini-3-flash-preview',
  model_memory: 'gemini-3-flash-preview',
  model_conversation_summary: 'gemini-3-flash-preview',
  model_command_judge: DEFAULT_COMMAND_JUDGE_MODEL,
};

/** Per-key effort default. Each mirrors the one `engine::aux_purpose` applies,
 *  and each is the literal its call site hardcoded before the preference
 *  existed. The summary keeps `low` (ADR 0102's measurements say output length
 *  does not track it), and the rest spend nothing on deliberation. */
const BACKGROUND_REASONING_DEFAULTS: Record<BackgroundReasoningKey, string> = {
  reasoning_title: 'none',
  reasoning_image_description: 'none',
  reasoning_memory: 'none',
  reasoning_conversation_summary: 'low',
  reasoning_command_judge: 'none',
};

export function currentBackgroundModel(key: BackgroundModelKey): string {
  const fallback = key === 'model_conversation_summary'
    // Split out of `model_memory`, so an unset value follows whatever the user
    // pinned there. The engine resolves the same fallback.
    ? currentBackgroundModel('model_memory')
    : BACKGROUND_MODEL_DEFAULTS[key];
  if (preferences.value.status !== 'loaded') return fallback;
  return preferences.value.data[key] || fallback;
}

export function currentBackgroundReasoning(key: BackgroundReasoningKey): string {
  const fallback = BACKGROUND_REASONING_DEFAULTS[key];
  if (preferences.value.status !== 'loaded') return fallback;
  return preferences.value.data[key] || fallback;
}

/** Persist one background *model selection*, both halves.
 *
 *  Two writes, not one: `PUT /api/v1/preferences` takes a single key, and the
 *  pair is deliberately two keys (ADR 0107). A failure between them leaves a
 *  stale effort beside the new model, which nothing acts on: the picker clamps
 *  for display and `RoutingProvider::effort_for_model` clamps the request. The
 *  failed write toasts, so the user is not left guessing. */
export async function saveModelSelection(
  modelKey: BackgroundModelKey,
  reasoningKey: BackgroundReasoningKey,
  patch: { model: string; reasoningEffort: string | null },
): Promise<void> {
  await savePreference(modelKey, patch.model);
  if (patch.reasoningEffort !== null) await savePreference(reasoningKey, patch.reasoningEffort);
}

// --- Compose destination (coding-agent chip + hand-off hint) ---

/** The account default coding agent — the SEED for a fresh compose's backend
 *  chip (via `selectedCodingAgent`, set at `loadPreferences`). Workspace-scoped
 *  (not device-scoped). Default Claude Code — same as the engine's NULL fallback.
 *  Compose picks are per-draft (see `composeSelections`) and deliberately do NOT
 *  write this back (draft-only), so there is no `setCodingAgentDefault`. */
export function currentCodingAgentDefault(): CodingAgent {
  return currentPreference('coding_agent_default', ['claude-code', 'codex'], 'claude-code');
}

/** The two Claude Code permission modes Lucidos offers. CC has six; the other
 *  four are withheld deliberately (see the engine's `CcPermissionMode`). */
export const CC_PERMISSION_MODES = ['accept-edits', 'auto'] as const;
export type CcPermissionMode = (typeof CC_PERMISSION_MODES)[number];

/** Which of Claude Code's own permission modes coding-agent threads run in.
 *  Workspace-scoped. Defaults to `accept-edits`, the mode every session ran
 *  before the key existed, so the engine's NULL fallback and this agree. */
export function currentCodingAgentPermissionMode(): CcPermissionMode {
  return currentPreference(
    'coding_agent_claude_permission_mode',
    CC_PERMISSION_MODES,
    'accept-edits',
  );
}

export function setCodingAgentPermissionMode(mode: CcPermissionMode): Promise<void> {
  return savePreference('coding_agent_claude_permission_mode', mode);
}

// --- New-workspace welcome + starter suggestions ---

/** Whether the new-workspace welcome + starter suggestions have been retired
 *  via "Don't show this again". Treats not-yet-loaded (and failed) preferences
 *  as dismissed so a returning user who already retired it never sees a flash
 *  during the preferences fetch — an onboarding surface fails closed. A genuinely
 *  new workspace gets it back once the preference loads and reads unset. */
export function welcomeSuggestionsDismissed(): boolean {
  if (preferences.value.status !== 'loaded') return true;
  return preferences.value.data['welcome_suggestions_dismissed'] === 'true';
}

/** One-way retire — the user clicked "Don't show this again" on the
 *  new-workspace welcome. Idempotent: skips the write only when the LOADED
 *  preference already says dismissed. */
export function dismissWelcomeSuggestions(): Promise<void> {
  if (preferences.value.status === 'loaded'
    && preferences.value.data['welcome_suggestions_dismissed'] === 'true') {
    return Promise.resolve();
  }
  return savePreference('welcome_suggestions_dismissed', 'true');
}

// --- Backup reminder banner ---
//
// The banner asks "have you switched backup on?", and the answer is already in
// this map: `GET /backup/schedule` decides its `schedule` field as
// `is_schedule_active(cron) && provider.is_some()` (the engine's
// `api::backup::schedule_response`), and both are ordinary preference rows that
// `GET /preferences` returns. So the banner needs no endpoint of its own and no
// poll, and because `set_backup_schedule` writes through `PreferenceStore::set`
// (which announces `PreferencesChanged` → `loadPreferences`), enabling backup
// retracts it live on every connected device.
//
// Note the asymmetry on the engine side: that response's `provider` field is
// reported whether or not the schedule is active, because a destination does
// not stop existing when the cron is off. Only `schedule` needs both halves,
// which is the half these predicates mirror.
//
// Deliberately NOT backup *health*: a schedule that exists but whose runs are
// failing is the Settings health card's job. Keeping this to "is it on?" is what
// lets one dismissal mean one thing.

/** Mirror of the engine's `core::backup::is_schedule_active`: a schedule counts
 *  as active when it is neither empty nor the literal "off". */
export function isBackupScheduleActive(schedule: string | undefined): boolean {
  return !!schedule && schedule !== 'off';
}

/** Whether automatic backup is switched on, by the same rule the engine's
 *  `GET /backup/schedule` uses: an active cron AND a provider. Either half
 *  missing means off (a provider picked with no schedule backs nothing up). */
export function backupIsActive(prefs: Record<string, string>): boolean {
  return isBackupScheduleActive(prefs['backup_schedule']) && !!prefs['backup_provider'];
}

/** How long the FIRST dismissal silences the reminder. */
export const BACKUP_REMINDER_SNOOZE_MS = 30 * 24 * 60 * 60 * 1000;

/** The value a SECOND dismissal writes: silenced for good. */
export const BACKUP_REMINDER_FOREVER = 'forever';

/** Whether the recorded dismissal still hides the reminder at `now`.
 *
 *  An unparseable value reads as NOT dismissed. This is a data-loss warning, so
 *  garbage in the preference must fail towards showing it, and the next dismiss
 *  then counts as the first and overwrites the garbage with a real instant. */
export function backupReminderHiddenByDismissal(value: string | undefined, now: number): boolean {
  if (!value) return false;
  if (value === BACKUP_REMINDER_FOREVER) return true;
  const at = Date.parse(value);
  if (Number.isNaN(at)) return false;
  return now - at < BACKUP_REMINDER_SNOOZE_MS;
}

/** The value to write when the user dismisses.
 *
 *  Nothing valid recorded yet → the first dismissal, which records the instant
 *  and snoozes 30 days. Already carrying an instant → this is the second
 *  dismissal (the snooze must have expired for the banner to be back on screen),
 *  so silence it for good.
 *
 *  Already `forever` stays `forever`. Unreachable from the UI (a permanently
 *  dismissed banner is never on screen to dismiss again), but the alternative is
 *  a function that DOWNGRADES a permanent dismissal into a fresh 30-day snooze,
 *  which is the wrong direction for a silence the user asked for twice. */
export function backupReminderNextDismissal(value: string | undefined, now: number): string {
  if (value === BACKUP_REMINDER_FOREVER) return BACKUP_REMINDER_FOREVER;
  const dismissedOnce = !!value && !Number.isNaN(Date.parse(value));
  return dismissedOnce ? BACKUP_REMINDER_FOREVER : new Date(now).toISOString();
}

/** Pure core of the banner's visibility, over a loaded preference map. */
export function backupReminderVisibleIn(prefs: Record<string, string>, now: number): boolean {
  if (backupIsActive(prefs)) return false;
  return !backupReminderHiddenByDismissal(prefs['backup_reminder_dismissed'], now);
}

/** Whether the app-shell backup reminder belongs on screen right now. Fails
 *  CLOSED while preferences are unloaded or failed, so it never flashes during
 *  the startup fetch at a user who already silenced it (same reasoning as
 *  `welcomeSuggestionsDismissed`). */
export function backupReminderVisible(now: number = Date.now()): boolean {
  if (preferences.value.status !== 'loaded') return false;
  return backupReminderVisibleIn(preferences.value.data, now);
}

/** Record a dismissal: snooze on the first, silence for good on the second.
 *  A no-op while preferences are unloaded, which is unreachable from the UI
 *  (the banner is hidden in that state, so there is nothing to click). */
export function dismissBackupReminder(now: number = Date.now()): Promise<void> {
  if (preferences.value.status !== 'loaded') return Promise.resolve();
  const next = backupReminderNextDismissal(
    preferences.value.data['backup_reminder_dismissed'],
    now,
  );
  return savePreference('backup_reminder_dismissed', next);
}

// --- Command guard (ADR 0002) ---

/** Master toggle for the command guard (the bash/python safety gate). Defaults
 *  to false — the feature ships dark and is enabled per-workspace. Mirrors the
 *  backend `command_guard` preference (`core/preferences.rs`). */
export function currentCommandGuard(): boolean {
  if (preferences.value.status !== 'loaded') return false;
  return preferences.value.data['command_guard'] === 'true';
}

export function setCommandGuard(enabled: boolean): Promise<void> {
  return savePreference('command_guard', enabled ? 'true' : 'false');
}

/** Sub-toggle for the LLM judge — when off, the guard uses only the static
 *  "dangerous" list for the ask lane. Defaults to true (on when the guard is on).
 *  Only meaningful while the master `command_guard` toggle is on. */
export function currentCommandGuardJudge(): boolean {
  if (preferences.value.status !== 'loaded') return true;
  return preferences.value.data['command_guard_judge'] !== 'false';
}

export function setCommandGuardJudge(enabled: boolean): Promise<void> {
  return savePreference('command_guard_judge', enabled ? 'true' : 'false');
}

// --- Voice ---

/**
 * Whether this workspace has voice turned on at all.
 *
 * Off unless somebody opted in, matching the engine's `voice_enabled` default.
 * An unloaded preference set therefore reads OFF, which is the right way round:
 * the call control appears once we know it should, never in the gap before the
 * answer arrives.
 */
export function voiceEnabled(): boolean {
  if (preferences.value.status !== 'loaded') return false;
  return preferences.value.data['voice_enabled'] === 'true';
}

export function setVoiceEnabled(enabled: boolean): Promise<void> {
  return savePreference('voice_enabled', enabled ? 'true' : 'false');
}

/** The speech-to-speech model a *voice session* speaks through. Mirrors the
 *  backend `model_voice_talker` default in `core/preference_catalog.rs`.
 *  Deliberately NOT a chat-model registry row, so it is a typed id rather than
 *  a pick from `backgroundModelChoices()`: a realtime model cannot serve an
 *  ordinary turn and never appears in that registry. */
export const DEFAULT_VOICE_TALKER_MODEL = 'gpt-realtime';

/**
 * What is STORED, which is empty until somebody sets it.
 *
 * Deliberately not resolved against the default. The field renders this. A
 * resolved value would fill an unset field with the default. A clear would
 * then read as an edit and save an empty string on every blur. Empty is what
 * the placeholder is for, and the engine falls back on its own.
 */
export function storedVoiceTalkerModel(): string {
  if (preferences.value.status !== 'loaded') return '';
  return preferences.value.data['model_voice_talker'] ?? '';
}

export function setVoiceTalkerModel(model: string): Promise<void> {
  return savePreference('model_voice_talker', model.trim());
}

/** The model that turns the caller's speech into text inside the talker's
 *  socket. The second and last model in the voice loop: nothing translates and
 *  nothing summarises. Mirrors the backend `model_voice_transcriber` default. */
export const DEFAULT_VOICE_TRANSCRIBER_MODEL = 'gpt-4o-mini-transcribe';

export function storedVoiceTranscriberModel(): string {
  if (preferences.value.status !== 'loaded') return '';
  return preferences.value.data['model_voice_transcriber'] ?? '';
}

export function setVoiceTranscriberModel(model: string): Promise<void> {
  return savePreference('model_voice_transcriber', model.trim());
}

/** The voice a call is spoken in, as the provider's own name for one. Not a
 *  model, and not the language. Mirrors the backend `voice_talker_voice`. */
export const DEFAULT_VOICE_TALKER_VOICE = 'marin';

export function storedVoiceTalkerVoice(): string {
  if (preferences.value.status !== 'loaded') return '';
  return preferences.value.data['voice_talker_voice'] ?? '';
}

export function setVoiceTalkerVoice(voice: string): Promise<void> {
  return savePreference('voice_talker_voice', voice.trim());
}

/**
 * One section of the resident block, as the toggles need to draw it.
 *
 * What a call loads before it starts. The talker looks nothing up mid-call, so
 * this block is the whole of what it answers without waiting for the agent.
 *
 * A mirror of `SECTIONS` in `crates/lucidos-engine/src/voice/sections.rs`,
 * which is the registry: the ids, the headings and which ones ship on are all
 * decided there, beside the builders that fill them. A Rust test reads this
 * list and fails if the two drift, the way `voice::language` reads the Locale
 * dropdown.
 */
export const VOICE_RESIDENT_SECTIONS: readonly {
  id: string;
  title: string;
  onByDefault: boolean;
}[] = [
  { id: 'who-and-where', title: 'Who you are talking to, and when', onByDefault: true },
  { id: 'this-thread', title: 'This conversation so far', onByDefault: true },
  { id: 'workspace-shape', title: 'What this workspace has', onByDefault: true },
];

/** Write the whole list. Only {@link setVoiceSectionEnabled} calls it: the one
 *  place that knows what turning a single toggle does to the rest. */
function setVoiceResidentSections(sections: string): Promise<void> {
  return savePreference('voice_resident_sections', sections.trim());
}

/**
 * The ids a call opens with right now.
 *
 * `null` means nothing is stored, which is the default set. It is NOT the same
 * as the empty list: an empty stored value means the reader turned every
 * section off, and the engine reads it that way too.
 */
export function voiceResidentSelection(): string[] | null {
  if (preferences.value.status !== 'loaded') return null;
  const stored = preferences.value.data['voice_resident_sections'];
  if (stored === undefined) return null;
  return stored
    .split(',')
    .map((id) => id.trim())
    .filter(Boolean);
}

/** Is this section in the block a call would open with? */
export function voiceSectionEnabled(id: string): boolean {
  const selection = voiceResidentSelection();
  if (selection === null) {
    return VOICE_RESIDENT_SECTIONS.some((s) => s.id === id && s.onByDefault);
  }
  return selection.includes(id);
}

/**
 * Turn one section on or off, rewriting the whole stored list.
 *
 * The registry's order is what gets written, not the order they were toggled
 * in: the block reads better the same way twice, and the engine renders the
 * sections in its own order anyway.
 *
 * An id nothing in the registry carries is preserved. A newer engine may define
 * one this client does not know, and dropping it would silently turn it off.
 */
export function setVoiceSectionEnabled(id: string, on: boolean): Promise<void> {
  const current = new Set(
    voiceResidentSelection() ??
      VOICE_RESIDENT_SECTIONS.filter((s) => s.onByDefault).map((s) => s.id),
  );
  if (on) current.add(id);
  else current.delete(id);
  const registry = VOICE_RESIDENT_SECTIONS.map((s) => s.id);
  const ordered = registry.filter((known) => current.has(known));
  const unknown = [...current].filter((held) => !registry.includes(held));
  return setVoiceResidentSections([...ordered, ...unknown].join(','));
}

/**
 * Which microphone a call opens on THIS device.
 *
 * Device-scoped, because the value is a browser's own opaque handle for a
 * microphone. It means nothing in another browser and nothing on another
 * machine, which is exactly the scope a Lucidos device has.
 *
 * Empty means the system default, which is what every call did before this
 * existed. So an untouched workspace behaves as it always has.
 */
export function storedVoiceInputDevice(): string {
  if (preferences.value.status !== 'loaded') return '';
  return preferences.value.data['voice_input_device'] ?? '';
}

export function setVoiceInputDevice(deviceId: string): Promise<void> {
  return savePreference('voice_input_device', deviceId.trim(), undefined, true);
}
