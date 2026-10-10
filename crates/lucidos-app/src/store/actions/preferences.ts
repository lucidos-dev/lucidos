import { effect, signal, untracked } from '@preact/signals';
import { preferences, showToast, removeToast, notificationsFilter, NOTIFICATIONS_FILTER_STORAGE_KEY, currentModel, selectedCodingAgent, clampThreadDrawerWidth, llmConfigured, welcomeDismissed, type NotificationsFilter } from '../store';
import type { CodingAgent } from '../../api/types';
import type { ThreadState } from '../thread-events';
import { failedIfFresh } from '../types';
import { getPreferences, setPreference, isTransientFetchError, retryTransientRead, getTheme, ApiError } from '../../api/client';
import { resolvedHexColor } from '../../utils/cssColor';
import { getDeviceId } from './devices';
import { errorDetail } from '../../utils/errorDetail';
import { createFailureCounter } from '../../utils/failureCounter';
import { clampEffortFor, modelDefaultEffort } from './models';
import { parseChatEfforts, withChatEffort } from '../chatEfforts';
import { isIOSPwa, isTauri } from '../../utils/platform';
import { setProseAutocorrect } from '../../utils/noAutofill';
import { publishScrollbarGutter } from '../../utils/scrollbarGutter';
import { setTitlebarColor, windowReadyToShow } from '../../utils/tauri';
import {
  STYLE_OVERRIDES_KEY, STYLE_OVERRIDES_STORAGE_KEY, STYLE_RESET_PARAM,
  isAllowedOverride, parseStyleOverrides,
  serializeStyleOverrides, styleResetRequested,
} from '../../utils/styleOverrides';

import {
  EMPTY_THEME, FONT_PREFERENCES, THEME_EFFECTS_STORAGE_KEY,
  THEME_KEY, THEME_STORAGE_KEY,
  MOTION_STORAGE_KEY, SYSTEM_THEME_MODE_SETTLE_MS, THEME_MODE_ATTRIBUTE, THEME_MODE_BG,
  THEME_MODE_KEY, THEME_MODE_STORAGE_KEY, UI_SCALE_STORAGE_KEY,
  FONT_BOLD_ATTRIBUTE, FONT_FAMILY_STORAGE_KEY, WORKSPACE_FONT_STORAGE_KEY,
  clampUiScale, fontBoldMark, isWorkspaceFontId, themeBackground, themeTokenNames, parseResolvedTheme,
  parseUiScale, parseWorkspaceFont, replaceInlineTokens, resolveFont, resolveThemeMode,
  sanitizeResolvedTheme,
  type FontPreference, type ThemeEffectsPref, type MotionPref, type ResolvedTheme,
  type ResolvedThemeMode, type ThemeMode, type WorkspaceFont,
} from '@lucidos/appearance';
import { registerFontsInUse } from '@lucidos/font-faces';
import { dataMountUrl } from '../../api/client';
import { loadWorkspaceFonts, workspaceFontList } from './workspaceFonts';
import { motionPreference } from '../../utils/motion';
import { themeEffectsPreference } from '../../utils/themeEffects';
import { AUTOCORRECT_STORAGE_KEY } from '@lucidos/text-entry';
import {
  parseFlag, PREFERENCE_CATALOG, PREF_FONT_FAMILY, PREF_PROVIDER_ENABLED_CLOUDFLARE_WORKERS_AI,
  PREF_PROVIDER_ENABLED_SYSTEM_ONE_CUSTOM, PREF_PROVIDER_ENABLED_TYPESAFE, PREF_SYSTEM_ONE_CUSTOM_MODEL,
  PREF_SYSTEM_ONE_CUSTOM_URL, PREF_THEME, PREF_UI_SCALE,
  type PreferenceKey, type PreferenceValues,
} from '@lucidos/preference-catalog';
import { VOICE_RESIDENT_SECTIONS } from '@lucidos/engine-constants';

/** Re-exported so the components that already import these from the store keep
 *  one import site. The definitions live in the appearance contract, which is
 *  the single source the two FOUC scripts and the SDK read as well. */
export type { FontId, FontPreference, ThemeMode } from '@lucidos/appearance';
export {
  UI_SCALE_MIN, UI_SCALE_MAX, UI_SCALE_STEP, clampUiScale,
} from '@lucidos/appearance';

export const UI_SCALE_DEFAULT = Number(PREF_UI_SCALE.fallback);

/** What an unset `theme` means: the stylesheet as shipped, no inline tokens. */
export const DEFAULT_THEME_ID = PREF_THEME.fallback;

/** What an unset `font-family` means: follow the active theme, which falls back
 *  to the fallback font when the theme names no font. */
const DEFAULT_FONT_PREFERENCE: FontPreference = PREF_FONT_FAMILY.fallback;

export type ImageModel = PreferenceValues<'image_model'>;

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

let systemThemeModeQuery: MediaQueryList | null = null;
let systemThemeModeSettleTimer: number | null = null;
// Seeded so loadPreferences can skip a no-op applyThemeMode when unchanged. The
// matching module-init install of the OS listener sits beside
// `syncSystemThemeModeListener`, which cannot run before its own constants exist.
let lastAppliedThemeMode: ThemeMode = currentThemeMode();
/** The mode the page paints now, for surfaces that preview a mode (the theme
 *  picker). `applyThemeMode` sets it; seeded from what the boot script painted. */
export const paintedThemeMode = signal<ResolvedThemeMode>(
  document.documentElement.getAttribute(THEME_MODE_ATTRIBUTE) === 'light' ? 'light' : 'dark',
);
/** Bumped by every paint of scale, theme mode, theme, font or style overrides,
 *  after it writes the mirror the appearance boot script reads. App frames
 *  repaint off it (`app-appearance.ts`). */
export const appearanceVersion = signal(0);

// --- Generic helpers ---

/** A catalog entry as `currentPreference` reads it at runtime. */
interface CatalogEntry {
  readonly type: string;
  readonly values?: readonly string[];
  readonly inherits?: PreferenceKey;
  readonly fallback: string | null;
}

/** A key `currentPreference` resolves: one with a default, read as a string.
 *  A number key parses its own value, so it keeps its own reader. */
type ResolvableKey = {
  [K in PreferenceKey]: (typeof PREFERENCE_CATALOG)[K] extends {
    type: 'enum' | 'flag' | 'text';
    fallback: string;
  } ? K : never;
}[PreferenceKey];

/** What a key resolves to: one of its enum values, a flag's two, or text. */
type ResolvedPreference<K extends ResolvableKey> =
  (typeof PREFERENCE_CATALOG)[K] extends { values: readonly (infer V)[] } ? V
    : (typeof PREFERENCE_CATALOG)[K] extends { type: 'flag' } ? 'true' | 'false'
      : string;

/** A stored switch as the engine reads it: `'true'`, `'false'`, or null for a
 *  spelling it reads as neither. */
function flagReading(raw: string): 'true' | 'false' | null {
  const on = parseFlag(raw);
  return on === null ? null : on ? 'true' : 'false';
}

/** `raw` as a value of `key`, or null when the catalog does not accept it. */
function acceptedValue(key: PreferenceKey, raw: string | null | undefined): string | null {
  if (!raw) return null;
  const entry: CatalogEntry = PREFERENCE_CATALOG[key];
  if (entry.values) return entry.values.includes(raw) ? raw : null;
  if (entry.type === 'flag') return flagReading(raw);
  const text = raw.trim();
  return text === '' ? null : text;
}

/** A preference's value: the stored one, else the device mirror, else the key
 *  it inherits, else the catalog default. Values and default both come from the
 *  catalog, so a caller cannot supply a second copy of either. */
function currentPreference<K extends ResolvableKey>(key: K, localStorageKey?: string): ResolvedPreference<K> {
  if (preferences.value.status === 'loaded') {
    const stored = acceptedValue(key, preferences.value.data[key]);
    if (stored !== null) return stored as ResolvedPreference<K>;
  }
  if (localStorageKey) {
    const cached = acceptedValue(key, localStorage.getItem(localStorageKey));
    if (cached !== null) return cached as ResolvedPreference<K>;
  }
  const entry: CatalogEntry = PREFERENCE_CATALOG[key];
  if (entry.inherits) return currentPreference(entry.inherits as ResolvableKey) as ResolvedPreference<K>;
  return entry.fallback as ResolvedPreference<K>;
}

/** Take a preference's device-local mirror from what the engine just served.
 *  An absent or invalid value CLEARS it rather than leaving it: unset means the
 *  default, and a cache kept there would outlive a reset and keep answering for
 *  a preference nobody holds. */
function cacheServedValue(key: ResolvableKey, storageKey: string): void {
  const served = acceptedValue(key, preferences.value.status === 'loaded' ? preferences.value.data[key] : undefined);
  if (served !== null) {
    localStorage.setItem(storageKey, served);
  } else {
    localStorage.removeItem(storageKey);
  }
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

/** A preference write the engine has not accepted yet.
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

/** Writes parked once an immediate re-send has also failed transiently, and
 *  flushed on the next page resume. */
const pendingPreferenceWrites = new Map<string, PendingPreferenceWrite>();
let writeSeq = 0;

/** The newest `seq` requested per key, in-flight ones included. */
const latestWriteSeq = new Map<string, number>();

/** The newest write requested per key, until the engine answers it. A refetch
 *  that lands before the answer still holds the old value, so `loadPreferences`
 *  lays these back over it. */
const unansweredWrites = new Map<string, PendingPreferenceWrite>();

/** The `seq` of the last write the engine refused, per key. A refetch drops a
 *  write it saw unanswered once the engine has refused it. */
const refusedWriteSeq = new Map<string, number>();

function writeValues(writes: Iterable<[string, PendingPreferenceWrite]>): Record<string, string> {
  return Object.fromEntries([...writes].map(([key, write]) => [key, write.value]));
}

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
function settleDelivered(key: string, write: PendingPreferenceWrite): void {
  pendingPreferenceWrites.delete(key);
  if (unansweredWrites.get(key)?.seq === write.seq) unansweredWrites.delete(key);
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
      settleDelivered(key, write);
      refusedWriteSeq.set(key, write.seq);
      showToast(`Failed to save ${key} preference: ${errorDetail(e)}`, 'error', {
        key: PREFERENCE_REJECTED_TOAST,
      });
      return;
    }
    // Kept OUT of the try: a throw from the bookkeeping below is not a failed
    // save, and catching it here would report one.
    settleDelivered(key, write);
    return;
  }
  // Both attempts were cancelled, timed out, or dropped in transit. Park the
  // value and stay quiet: the user sees their change applied, and the resume
  // flush delivers it.
  pendingPreferenceWrites.set(key, write);
  writeFailures.recordFailure();
}

/** Re-send every parked preference write. Called from `startClient`'s resume
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
  unansweredWrites.clear();
  refusedWriteSeq.clear();
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
  const write = { value, deviceScoped, seq: ++writeSeq };
  // Claim the key BEFORE queueing, so any write already in flight or waiting its
  // turn can see it has been superseded and stand down.
  latestWriteSeq.set(key, write.seq);
  unansweredWrites.set(key, write);
  await deliverOrPark(key, write);
}

// --- UI scale ---

/** Whether a measurement pass is already owed this frame. */
let scaleMeasurementPending = false;

/** The two quantities DERIVED from the root font size, so both have to run
 *  after something moves it.
 *
 *  The scrollbar gutter is published in px while our `::-webkit-scrollbar` width
 *  is authored in rem. Un-remeasured, the composer stops lining up with the
 *  transcript at any scale but the one live at boot. The thread drawer's floor
 *  is what its rem-authored header row needs, so scaling up can leave a settled
 *  drawer narrower than its own header. */
function measureScaleDerivedValues(): void {
  scaleMeasurementPending = false;
  publishScrollbarGutter();
  clampThreadDrawerWidth();
}

/** Owe one measurement pass, on the next frame, however many callers ask.
 *
 *  Both measurements READ layout back right after a write that dirtied it, which
 *  forces a synchronous layout of the whole document. One is the honest price of
 *  the answer. One PER INPUT EVENT is what wedged the tab. The read denies the
 *  browser its batching. Every notch of a zoom gesture then paid a full layout,
 *  at a root font size nothing had laid out at before.
 *
 *  Deferring is safe because neither value has this as its only writer. The boot
 *  publish comes from `main.tsx`, `ThreadView` republishes on mount
 *  (`utils/scrollbarGutter.ts`), and `store.ts`'s module init already clamps the
 *  persisted drawer width. See
 *  `docs/plans/2026-09-19-zooming-cannot-wedge-the-tab.md`. */
function scheduleScaleMeasurements(): void {
  if (scaleMeasurementPending) return;
  if (typeof requestAnimationFrame !== 'function') {
    measureScaleDerivedValues();
    return;
  }
  // Claimed BEFORE the call, never after. A synchronous `requestAnimationFrame`
  // runs the callback first, so a later assignment parks the flag at `true` and
  // drops every pass after this one.
  scaleMeasurementPending = true;
  requestAnimationFrame(measureScaleDerivedValues);
}

export function applyUiScale(scale: number): void {
  const clamped = clampUiScale(scale);
  localStorage.setItem(UI_SCALE_STORAGE_KEY, String(clamped));
  document.documentElement.style.setProperty('--user-ui-scale', `${clamped}%`);
  // It leads the measurement rather than trailing it, because an override may
  // retune the type scale the two quantities are measured against.
  reapplyStyleOverrides();
  scheduleScaleMeasurements();
  // The macOS traffic lights are centred on the header bar, whose height this
  // just changed, and nothing here tells the shell: `watchTitlebarBand` observes
  // the rendered band and pushes for every mover, this one included.
  appearanceVersion.value++;
}

export function currentUiScale(): number {
  if (preferences.value.status !== 'loaded') return UI_SCALE_DEFAULT;
  const raw = preferences.value.data['ui-scale'] || preferences.value.data['text-size'] || preferences.value.data['font-size'];
  // `parseUiScale` answers null for nothing usable, which for a SETTING means
  // the default. The FOUC scripts want that null instead, so they can leave
  // --user-ui-scale unset and let the stylesheet's own fallback answer.
  return parseUiScale(raw) ?? UI_SCALE_DEFAULT;
}

/** A scale painted ahead of its save. The scale panel saves once a gesture
 *  settles, and a refetch before then must keep painting this one. */
let previewedUiScale: number | null = null;

/** Paint a scale the user is still choosing, without saving it. */
export function previewUiScale(scale: number): void {
  previewedUiScale = clampUiScale(scale);
  applyUiScale(previewedUiScale);
}

/** Drop a preview that will not be saved, and paint the saved scale again. */
export function cancelUiScalePreview(): void {
  const previewed = previewedUiScale;
  previewedUiScale = null;
  if (previewed !== null && previewed !== currentUiScale()) applyUiScale(currentUiScale());
}

export function setUiScale(scale: number): Promise<void> {
  const clamped = clampUiScale(scale);
  previewedUiScale = null;
  return savePreference('ui-scale', String(clamped), () => applyUiScale(clamped), true);
}

// --- Theme mode ---

/** Whether the OS is asking for light right now. The one read point, so a
 *  breadcrumb can never record a different sample than the one that painted. */
function osPrefersLight(): boolean {
  return window.matchMedia('(prefers-color-scheme: light)').matches;
}

/** Marks <html> while a theme or mode swap paints. base.css turns every
 *  transition off under it, so no colour eases from the old theme to the new
 *  while the rest of the page snaps. */
const THEME_SWAP_ATTRIBUTE = 'data-theme-swap';
let themeSwapCount = 0;

/** The mark must outlast the first frame, because the browser paints the swap
 *  after that frame's callbacks. It lifts on the second, once nothing is left to
 *  ease. */
function markThemeSwap(): void {
  if (typeof requestAnimationFrame !== 'function') return;
  const root = document.documentElement;
  const swap = ++themeSwapCount;
  root.setAttribute(THEME_SWAP_ATTRIBUTE, '');
  requestAnimationFrame(() => requestAnimationFrame(() => {
    if (swap === themeSwapCount) root.removeAttribute(THEME_SWAP_ATTRIBUTE);
  }));
}

export function applyThemeMode(mode: ThemeMode): void {
  markThemeSwap();
  const prefersLight = osPrefersLight();
  const resolved = resolveThemeMode(mode, prefersLight);
  // The theme is per mode, so it is re-laid on every mode apply. Stale names
  // go first, so the mode's own inline background below is never removed.
  const themeTokens = activeTheme[resolved];
  clearStaleThemeTokens(themeTokens);
  paintedThemeMode.value = resolved;
  const bg = themeBackground(themeTokens) ?? THEME_MODE_BG[resolved];
  // Theme mode flash telemetry. index.html installs __themeModeLogEvt as a fetch shim
  // that POSTs to /api/v1/internal/client-log (engine.log breadcrumbs).
  type ThemeModeLogEvt = (label: string, info: unknown) => void;
  const logEvt = (window as unknown as { __themeModeLogEvt?: ThemeModeLogEvt }).__themeModeLogEvt;
  if (logEvt) {
    logEvt('applyThemeMode', {
      input: mode,
      resolved,
      priorDataThemeMode: document.documentElement.getAttribute(THEME_MODE_ATTRIBUTE),
      mqLight: prefersLight,
    });
  }
  localStorage.setItem(THEME_MODE_STORAGE_KEY, mode);
  document.documentElement.setAttribute(THEME_MODE_ATTRIBUTE, resolved);
  document.documentElement.style.setProperty('--bg-primary', bg);
  // Mirrors the inline FOUC IIFE in index.html — keeps <html> covered on
  // toggle and on the next cold reload, before global.css re-applies its
  // `html { background: var(--bg-primary); }` rule.
  document.documentElement.style.background = bg;

  setThemeTokens(themeTokens);
  const themeActive = Object.keys(themeTokens).length > 0;
  // A theme background that is not a hex literal still has to reach the canvas.
  // The inline literal above would hide it, so the var takes its place.
  if (themeTokens['--bg-primary'] && !themeBackground(themeTokens)) {
    document.documentElement.style.background = 'var(--bg-primary)';
  }

  const meta = document.querySelector('meta[name="theme-color"]');
  if (meta) {
    meta.setAttribute('content', (themeActive && resolvedHexColor('var(--bg-primary)')) || bg);
  }

  document.documentElement.style.colorScheme = resolved;

  // Tauri (packaged macOS): match the reclaimed title-bar band's behind-the-
  // webview fallback (the window background) to the in-app header — the
  // header-gradient top stop per mode (mirrors --header-gradient in
  // styles/global/base.css, like the --bg-primary literal above; the visible
  // band itself is the CSS .titlebar-strip). Best-effort and cosmetic: it runs
  // whenever the mode is applied (incl. startup / OS appearance changes) with no
  // user-facing surface, and a failed call self-heals on the next applyThemeMode, so
  // a toast would be wrong.
  if (isTauri()) {
    const defaultTitlebar = resolved === 'light' ? '#1a6fd0' : '#15549e';
    const titlebar = (themeActive && resolvedHexColor('var(--titlebar-strip-bg)')) || defaultTitlebar;
    setTitlebarColor(titlebar).catch((e) => console.warn('[titlebar] tint failed', e));
    // The mode is resolved and on the document, so a window shown now shows a
    // page in the user's appearance. The shell keeps the launch window hidden until
    // it hears this. One-shot inside the wrapper, since this line also runs on
    // every toggle and system-appearance change.
    windowReadyToShow();
  }

  syncSystemThemeModeListener(mode);
  lastAppliedThemeMode = mode;
  // This just wrote --bg-primary and the theme inline and swapped the token
  // block wholesale, so any override of a themed token goes back on top.
  reapplyStyleOverrides();
  appearanceVersion.value++;
}

// --- Theme (a named set of token values, docs/plans/2026-09-26-looks.md) ---
//
// The engine resolves a theme, derivation included, and serves one map per
// theme mode plus the fonts it suggests. This module keeps the active theme and
// caches it for the boot script. `applyThemeMode` lays the current mode's map
// inline on <html>, and `applyFontFamily` resolves the UI font against it. The
// picker's list of themes lives in `store/actions/themes.ts`.

/** Seeded from the boot cache, so the first apply paints what boot painted. */
let activeTheme: ResolvedTheme = parseResolvedTheme(localStorage.getItem(THEME_STORAGE_KEY));
/** The id last asked for. Null until this page asked, so the first preference
 *  load always fetches. */
let requestedThemeId: string | null = null;
/** Every name the boot script may have set from the same cache, so switching
 *  to a theme without one of them still removes it. */
let appliedThemeNames: string[] = themeTokenNames(activeTheme);
let themeLoadSeq = 0;

function clearStaleThemeTokens(next: Record<string, string>): void {
  const root = document.documentElement;
  for (const name of appliedThemeNames) {
    if (!(name in next)) root.style.removeProperty(name);
  }
}

function setThemeTokens(tokens: Record<string, string>): void {
  const root = document.documentElement;
  for (const [name, value] of Object.entries(tokens)) root.style.setProperty(name, value);
  appliedThemeNames = Object.keys(tokens);
}

export function currentThemeId(): string {
  if (preferences.value.status === 'loaded') {
    const raw = preferences.value.data[THEME_KEY];
    if (raw) return raw;
  }
  return DEFAULT_THEME_ID;
}

/** Paint `theme` now, and cache it for the next cold start. The UI font is
 *  re-resolved too, since a device that follows the theme takes its font. Not
 *  before the preferences load: the default would mask an explicit pick. The
 *  load applies the font itself. */
function applyTheme(theme: ResolvedTheme): void {
  activeTheme = theme;
  localStorage.setItem(THEME_STORAGE_KEY, JSON.stringify(theme));
  applyThemeMode(lastAppliedThemeMode);
  if (preferences.value.status === 'loaded') applyFontFamily(currentFontFamily());
}

/** The refused theme this page already warned about, so a background refresh
 *  on every preference load does not toast again. */
let refusedThemeWarned: string | null = null;

/**
 * Fetch the active theme and paint it. Called when the `theme` preference loads
 * or changes, and when the active theme's file is written or deleted.
 *
 * A theme that no longer exists paints the default, and so does one the engine
 * now refuses, with a warning. Any other failure keeps what is painted, so a
 * transient failure never flashes the default. The next preference load asks
 * again. `picked` marks a theme the user just chose, who is owed a toast
 * whenever it cannot load. `known` is a map already in hand, such as the
 * picker's. It paints at once, and the fetch then confirms it.
 */

export async function refreshActiveTheme(
  id: string = currentThemeId(),
  picked = false,
  known?: ResolvedTheme,
): Promise<void> {
  const seq = ++themeLoadSeq;
  requestedThemeId = id;
  if (id === DEFAULT_THEME_ID) {
    applyTheme(EMPTY_THEME);
    return;
  }
  if (known) applyTheme(sanitizeResolvedTheme(known));
  try {
    const theme = await getTheme(id);
    if (seq !== themeLoadSeq) return;
    if (refusedThemeWarned === id) refusedThemeWarned = null;
    const fetched = sanitizeResolvedTheme(theme.resolved);
    // Repainting the map a pick already painted would cut off every running
    // transition, and change nothing.
    if (!known || JSON.stringify(fetched) !== JSON.stringify(activeTheme)) applyTheme(fetched);
  } catch (e) {
    if (seq !== themeLoadSeq) return;
    if (e instanceof ApiError && (e.httpCode === 404 || e.httpCode === 400)) {
      applyTheme(EMPTY_THEME);
      return;
    }
    // The file exists but the engine refuses it, such as a theme saved before
    // a validation rule (ADR 0309). Its cached map must stop painting, and the
    // user must learn why the theme changed.
    if (e instanceof ApiError && e.httpCode === 422) {
      applyTheme(EMPTY_THEME);
      if (picked || refusedThemeWarned !== id) {
        refusedThemeWarned = id;
        showToast(`The theme "${id}" no longer passes validation, so the default theme shows: ${errorDetail(e)}`, 'warning');
      }
      return;
    }
    // Forget the request, so the next preference load asks again.
    requestedThemeId = null;
    if (picked && !known) {
      showToast(`Could not load the theme "${id}": ${errorDetail(e)}`, 'error');
      return;
    }
    // Carve-out: best-effort telemetry (.claude/rules/frontend.md). This is a
    // background refresh after an SSE event or a preference load, or a pick
    // that already shows its theme. The next preference load asks again.
    console.warn(`[themes] could not load theme "${id}"`, e);
  }
}

/** Fetch only when the preference names a theme other than the one asked for. */
function refreshActiveThemeIfChanged(): void {
  if (currentThemeId() === requestedThemeId) return;
  void refreshActiveTheme();
}

/** Whether a `data/` path is the active theme's file, for the SSE arms that
 *  refresh it on a write or a delete. */
export function isActiveThemePath(path: string | undefined): boolean {
  return path === `themes/${currentThemeId()}.json`;
}

export function setTheme(id: string, known?: ResolvedTheme): Promise<void> {
  // The id is passed on, because `savePreference` runs the side effect before
  // it updates the signal `currentThemeId` reads.
  return savePreference(THEME_KEY, id, () => void refreshActiveTheme(id, true, known), true);
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
const SYSTEM_THEME_MODE_RESUME_EVENTS: ReadonlyArray<readonly [EventTarget, string]> = [
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
function refreshSystemThemeMode(): void {
  if (currentThemeMode() !== 'system') return;
  if (document.visibilityState !== 'visible') return;
  const resolved = resolveThemeMode('system', osPrefersLight());
  if (document.documentElement.getAttribute(THEME_MODE_ATTRIBUTE) === resolved) return;
  applyThemeMode('system');
}

/** Arm one shared settle timer, which re-READS the OS when it fires. Nothing
 *  ever applies the value an event carried: a flip that raced the visibility
 *  guard has been corrected by the time this samples.
 *
 *  An already-armed timer is left alone rather than pushed back. A burst then
 *  resolves one settle delay after its first event, not after its last. */
function scheduleSystemThemeModeRefresh(): void {
  if (systemThemeModeSettleTimer !== null) return;
  systemThemeModeSettleTimer = window.setTimeout(() => {
    systemThemeModeSettleTimer = null;
    refreshSystemThemeMode();
  }, SYSTEM_THEME_MODE_SETTLE_MS);
}

/** Subscribe to the OS appearance while the preference is `system`, and to
 *  nothing at all otherwise. Called from every `applyThemeMode`, so it tears the
 *  previous registration down first and is safe to run repeatedly. */
function syncSystemThemeModeListener(mode: ThemeMode): void {
  systemThemeModeQuery?.removeEventListener('change', scheduleSystemThemeModeRefresh);
  systemThemeModeQuery = null;
  for (const [target, type] of SYSTEM_THEME_MODE_RESUME_EVENTS) {
    target.removeEventListener(type, scheduleSystemThemeModeRefresh);
  }
  if (systemThemeModeSettleTimer !== null) {
    clearTimeout(systemThemeModeSettleTimer);
    systemThemeModeSettleTimer = null;
  }
  if (mode !== 'system') return;

  systemThemeModeQuery = window.matchMedia('(prefers-color-scheme: light)');
  systemThemeModeQuery.addEventListener('change', scheduleSystemThemeModeRefresh);
  for (const [target, type] of SYSTEM_THEME_MODE_RESUME_EVENTS) {
    target.addEventListener(type, scheduleSystemThemeModeRefresh);
  }
}

// Module-init install. loadPreferences skips applyThemeMode when the stored mode
// already matches lastAppliedThemeMode. Without this call a user on `system` would
// never get the OS listener attached.
syncSystemThemeModeListener(lastAppliedThemeMode);

/** The device's theme mode, defaulting to `system` (follow the OS light/dark
 *  setting). A device that has explicitly picked light or dark keeps its pick:
 *  this is only what applies when nothing is stored, which is also why changing
 *  it reaches existing devices that never opened Settings.
 *
 *  The default is mirrored in the FOUC script (index.html), the iframe FOUC
 *  script (api/sdk_prefs.rs), the SDK (`resolveThemeModePreference`) and the
 *  preference catalog. They paint at different moments of one page load, so a
 *  disagreement between them is a visible flash. */
export function currentThemeMode(): ThemeMode {
  // localStorage fallback matches the FOUC prevention script in index.html.
  // Covers: backend missing the preference (device_id change, save failure),
  // and the loading window before the API responds.
  return currentPreference(THEME_MODE_KEY, THEME_MODE_STORAGE_KEY);
}

export function setThemeMode(mode: ThemeMode): Promise<void> {
  return savePreference(THEME_MODE_KEY, mode, () => applyThemeMode(mode), true);
}

// --- Font family ---

/** The workspace fonts this page can paint: the installed list once it has
 *  loaded, else the entry the boot script painted from, then the theme's own. */
function knownWorkspaceFonts(): WorkspaceFont[] {
  const list = workspaceFontList.value;
  const cached = parseWorkspaceFont(localStorage.getItem(WORKSPACE_FONT_STORAGE_KEY));
  const installed = list.status === 'loaded' ? list.data.fonts : cached ? [cached] : [];
  return [...installed, ...activeTheme.workspace_fonts];
}

/** Paint the font a preference resolves to: the user's pick, else the active
 *  theme's, else the fallback. The RAW preference is what the boot script reads,
 *  so it resolves against its cached theme the same way on the next cold start.
 *
 *  A catalog font needs no loading: the bundle declares every face, and a face
 *  downloads once text uses it (ADR 0303). A workspace font registers its faces
 *  here, and its entry is cached for the boot script (ADR 0308). */
export function applyFontFamily(preference: FontPreference): void {
  const known = knownWorkspaceFonts();
  const font = resolveFont(preference, activeTheme.fonts, known);
  localStorage.setItem(FONT_FAMILY_STORAGE_KEY, preference);
  if (font.workspaceFont) {
    localStorage.setItem(WORKSPACE_FONT_STORAGE_KEY, JSON.stringify(font.workspaceFont));
  } else if (workspaceFontList.value.status === 'loaded') {
    // Only an authoritative list may clear the cache. Before it loads, a picked
    // font is simply not known yet.
    localStorage.removeItem(WORKSPACE_FONT_STORAGE_KEY);
  }
  registerFontsInUse(font, known, activeTheme.fonts.mono, dataMountUrl);
  document.documentElement.style.setProperty('--font-ui', font.stack);
  document.documentElement.style.setProperty('--font-features-text', font.features.text);
  document.documentElement.style.setProperty('--font-features-code', font.features.code);
  document.documentElement.setAttribute(FONT_BOLD_ATTRIBUTE, fontBoldMark(font));
  // This just wrote --font-ui and the two feature properties inline.
  reapplyStyleOverrides();
  appearanceVersion.value++;
}

/** The device's font preference, which may be `theme`. The default and the
 *  valid set come from the appearance contract, so this and the boot scripts
 *  cannot disagree.
 *
 *  Deliberately NOT backed by localStorage the way the theme is: `applyFontFamily`
 *  writes `lucidos-font-family` on every load for the FOUC script to read, so a
 *  cached value here would just be the previous default echoed back and would
 *  outlive a change to it. */
export function currentFontFamily(): FontPreference {
  if (preferences.value.status === 'loaded') {
    const raw = preferences.value.data['font-family'];
    if (isWorkspaceFontId(raw)) return raw;
    const known = FONT_PREFERENCES.find((font) => font === raw);
    if (known) return known;
  }
  return DEFAULT_FONT_PREFERENCE;
}

// A picked workspace font paints once the list naming it has loaded, and again
// whenever it changes: installed, removed, or shipped by a plugin.
effect(() => {
  if (workspaceFontList.value.status !== 'loaded') return;
  untracked(() => {
    if (preferences.value.status === 'loaded') applyFontFamily(currentFontFamily());
  });
});

export function setFontFamily(font: FontPreference): Promise<void> {
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
  // A cleared name uncovers the theme's value rather than deleting it.
  const valid: Record<string, string> = {};
  for (const [name, value] of Object.entries(map)) {
    if (isAllowedOverride(name, value)) valid[name] = value;
  }
  appliedOverrideNames = replaceInlineTokens(
    root.style, appliedOverrideNames, valid, activeTheme[paintedThemeMode.value],
  );
  localStorage.setItem(STYLE_OVERRIDES_STORAGE_KEY, serializeStyleOverrides(map));
  appearanceVersion.value++;
  // A retuned --font-size-* or spacing token moves the root font size's
  // consumers. So the remote owes the same two derived measurements a scale
  // change does, through the same frame coalescer: it can write on every
  // keystroke of a tuning session.
  scheduleScaleMeasurements();
  // The bar the macOS traffic lights are centred on moves here too, with a
  // retuned --desktop-bar-height. `watchTitlebarBand` is what
  // tells the shell, by observing the band rather than the writers.
}

/** Re-assert the overrides after something else has written inline tokens.
 *  `applyThemeMode` writes `--bg-primary`, which the remote may override, and calls
 *  this at the end so the override keeps winning. Without it a system-theme
 *  flip silently reverts a tuned background. The UI scale and font writers call
 *  it too. Their own properties are reserved (ADR 0309), but an override may
 *  still retune the type scale their measurements read. */
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
      // The theme goes too: any app can write one, so it is a second way to
      // make the UI unreadable, and this is the way out of both.
      if (currentThemeId() !== DEFAULT_THEME_ID) {
        void setTheme(DEFAULT_THEME_ID).catch((e) => console.warn('[style-remote] theme reset write failed', e));
      }
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
  return currentPreference('image_model');
}

export function setImageModel(model: ImageModel): Promise<void> {
  return savePreference('image_model', model);
}

// --- Notifications filter ---

export function currentNotificationsFilter(): NotificationsFilter {
  return currentPreference('notifications_filter', NOTIFICATIONS_FILTER_STORAGE_KEY);
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

/** Whether a notification may pop a toast over what the user is doing. */
export function currentNotificationToasts(): boolean {
  return currentPreference('notification_toasts', NOTIFICATION_TOASTS_KEY) === 'true';
}

export function setNotificationToasts(enabled: boolean): Promise<void> {
  const value = enabled ? 'true' : 'false';
  return savePreference('notification_toasts', value, () => {
    localStorage.setItem(NOTIFICATION_TOASTS_KEY, value);
  });
}

// --- Autocorrect ---

/** Whether this device's prose fields autocorrect. A stored value wins on any
 *  client; unset falls to the mirror, then to the catalog default. The mirror
 *  exists for the same reason the toasts switch keeps one: the composer can
 *  take focus before preferences load, and iOS reads the attribute at focus. */
export function currentAutocorrect(): boolean {
  return currentPreference('autocorrect', AUTOCORRECT_STORAGE_KEY) === 'true';
}

export function setAutocorrect(enabled: boolean): Promise<void> {
  const value = enabled ? 'true' : 'false';
  return savePreference('autocorrect', value, () => {
    localStorage.setItem(AUTOCORRECT_STORAGE_KEY, value);
    setProseAutocorrect(enabled);
  }, true);
}

// --- Motion ---

/** This device's motion preference. A stored value wins; unset falls to the
 *  mirror, then to the catalog default. The mirror is what the boot script
 *  reads, so first paint and this agree. */
export function currentMotion(): MotionPref {
  return currentPreference('motion', MOTION_STORAGE_KEY);
}

export function setMotion(pref: MotionPref): Promise<void> {
  return savePreference('motion', pref, () => {
    localStorage.setItem(MOTION_STORAGE_KEY, pref);
    motionPreference.value = pref;
  }, true);
}

// --- Theme effects ---

/** This device's theme-effects preference, resolved like motion: a stored value,
 *  else the mirror the boot script reads, else the catalog default. */
export function currentThemeEffects(): ThemeEffectsPref {
  return currentPreference('theme-effects', THEME_EFFECTS_STORAGE_KEY);
}

export function setThemeEffects(pref: ThemeEffectsPref): Promise<void> {
  return savePreference('theme-effects', pref, () => {
    localStorage.setItem(THEME_EFFECTS_STORAGE_KEY, pref);
    themeEffectsPreference.value = pref;
  }, true);
}

// --- Chat model & reasoning effort ---

/** The user's chat model preference. Unlike most preferences this is NOT
 *  validated against a fixed allow-list — the model set is now the DB-backed
 *  registry (user-extensible), and `RoutingProvider` resolves any id (with a
 *  prefix fallback), so any stored non-empty value is honored. */
export function currentChatModel(): string {
  return currentPreference('chat_model');
}

/** The stored `chat_model`, or null while unset and the router's own runs. */
export function storedChatModel(): string | null {
  if (preferences.value.status !== 'loaded') return null;
  return acceptedValue('chat_model', preferences.value.data.chat_model);
}

/** The stored `chat_reasoning_efforts` list, or null while unset. */
function storedChatEfforts(): string | null {
  if (preferences.value.status !== 'loaded') return null;
  return acceptedValue('chat_reasoning_efforts', preferences.value.data.chat_reasoning_efforts);
}

/** The tier a new chat on `modelId` runs at: the account's tier for it, else
 *  its *default effort*, snapped onto what the model offers. `null` means the
 *  engine sends no effort and the model runs at its provider's default.
 *  Mirrors `PreferenceStore::resolve_chat_overrides_for_thread`. */
export function accountChatEffort(modelId: string): string | null {
  const stored = parseChatEfforts(storedChatEfforts()).get(modelId);
  return stored === undefined ? modelDefaultEffort(modelId) : clampEffortFor(stored, modelId);
}

/** Persist the chat *model selection*, both halves.
 *
 *  One pick sets the pair, and the tier is stored for that model alone, so
 *  every other model keeps its own. `null` is the Default row, which removes
 *  the model's stored tier so it runs at its default effort. */
export async function setChatModelSelection(
  patch: { model: string; reasoningEffort: string | null },
): Promise<void> {
  await savePreference('chat_model', patch.model, () => { currentModel.value = patch.model; });
  // The list is written whole, so writing it unread would drop every other
  // model's tier.
  if (preferences.value.status !== 'loaded') {
    if (patch.reasoningEffort !== null) {
      showToast('Preferences have not loaded, so the reasoning effort was not saved', 'error');
    }
    return;
  }
  const stored = storedChatEfforts();
  const next = withChatEffort(stored, patch.model, patch.reasoningEffort);
  if (next !== (stored ?? '')) await savePreference('chat_reasoning_efforts', next);
}

// --- Response style ---

/** The id of the selected *response style*, or Standard when unset.
 *
 *  Deliberately NOT validated against a fixed list, for the reason
 *  `currentChatModel` is not: the user may add styles, so the set is open. An
 *  id nothing defines resolves to Standard in the engine, which is also what
 *  the picker shows for it. */
export function currentResponseStyle(): string {
  return currentPreference('response_style');
}

export function setResponseStyle(id: string): Promise<void> {
  return savePreference('response_style', id);
}

/** The value that clears the level, which is also its default. */
export const TECHNICAL_LITERACY_NOT_SET = PREFERENCE_CATALOG.technical_literacy.fallback;

/** The response style's second part: every catalog value but the one that
 *  clears it. */
export type TechnicalLiteracy = Exclude<PreferenceValues<'technical_literacy'>, typeof TECHNICAL_LITERACY_NOT_SET>;
export const TECHNICAL_LITERACY_LEVELS: readonly TechnicalLiteracy[] = PREFERENCE_CATALOG.technical_literacy.values
  .filter((level): level is TechnicalLiteracy => level !== TECHNICAL_LITERACY_NOT_SET);

/** A retired level that reads as `non-technical`, as it does in the engine. */
const MERGED_EVERYDAY = 'everyday';

/** The stored level, or `null` when unset. An unknown value reads as unset,
 *  which is also what the engine does with it. */
export function currentTechnicalLiteracy(): TechnicalLiteracy | null {
  if (preferences.value.status !== 'loaded') return null;
  const v = preferences.value.data['technical_literacy']?.trim();
  if (v === MERGED_EVERYDAY) return 'non-technical';
  return TECHNICAL_LITERACY_LEVELS.find((level) => level === v) ?? null;
}

/** `null` clears the level. */
export function setTechnicalLiteracy(level: TechnicalLiteracy | null): Promise<void> {
  return savePreference('technical_literacy', level ?? TECHNICAL_LITERACY_NOT_SET);
}

// --- Max tool calls (the per-turn tool-call cap) ---

export const MAX_TOOL_CALLS_DEFAULT = Number(PREFERENCE_CATALOG.max_tool_calls.fallback);

/** The floor rules out only the value that is broken rather than small. The
 *  loop checks `iterations > cap` after incrementing, so a cap of 0 would end
 *  the turn before the first LLM call. */
export const MAX_TOOL_CALLS_MIN = PREFERENCE_CATALOG.max_tool_calls.min;

/** The largest cap the engine accepts. It sits far below
 *  `Number.MAX_SAFE_INTEGER`, so every cap up to it survives `Number` exactly. */
export const MAX_TOOL_CALLS_MAX = PREFERENCE_CATALOG.max_tool_calls.max;

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
 *  unparseable value shows the default, and a parsed value is held between
 *  the catalog bounds. */
export function currentMaxToolCalls(): number {
  if (preferences.value.status !== 'loaded') return MAX_TOOL_CALLS_DEFAULT;
  const raw = preferences.value.data['max_tool_calls'];
  if (raw == null) return MAX_TOOL_CALLS_DEFAULT;
  // The engine's own rule (`Pref<Number>::resolve`): a number inside the
  // catalog bounds, truncated as the engine casts it; anything else is unset.
  const parsed = Number(raw.trim());
  if (raw.trim() === '' || !(parsed >= MAX_TOOL_CALLS_MIN && parsed <= MAX_TOOL_CALLS_MAX)) {
    return MAX_TOOL_CALLS_DEFAULT;
  }
  return Math.trunc(parsed);
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
// fetch. Only the newest ISSUED call's outcome is applied.
//
// An older call resolves with the newest one rather than early. Callers chain
// work on the promise (startup re-derives the update surfaces), and that work
// must find the preferences loaded. Waiting forward never waits on a hung
// fetch for long: the next resume issues a newer call, and the chain moves on.
let preferencesLoadSeq = 0;
let newestPreferencesLoad: Promise<void> = Promise.resolve();

export function loadPreferences(): Promise<void> {
  const load = loadPreferencesAs(++preferencesLoadSeq);
  newestPreferencesLoad = load;
  return load;
}

async function loadPreferencesAs(mySeq: number): Promise<void> {
  // Only flip to 'loading' on the first fetch — refetches (e.g. after an SSE
  // PreferencesChanged) keep showing existing data through the network round
  // trip and swap atomically when the response lands. Without this guard,
  // every preference toggle wipes subscribers to defaults until the GET
  // completes, which the user sees as a flash.
  if (preferences.value.status === 'not-loaded') {
    preferences.value = { status: 'loading' };
  }
  try {
    // Taken before the read: a write the engine accepts while it is out is
    // newer than its answer.
    const unansweredAtStart = [...unansweredWrites];
    // Retry a transient rejection before flipping to `failed`, same as
    // `loadRepositories` (repositoriesLoader.ts). Nothing re-triggers this
    // load once it fails (SSE only re-fires an already-`loaded` value): a
    // single cancelled startup fetch would otherwise paint every setting at
    // its default for the rest of the page load.
    const res = await retryTransientRead(() => getPreferences(getDeviceId()));
    // A newer call was issued while this one was in flight: its outcome
    // wins, so applying this stale one would overwrite fresher data.
    if (mySeq !== preferencesLoadSeq) return newestPreferencesLoad;
    preferences.value = {
      status: 'loaded',
      data: {
        ...res.preferences,
        ...writeValues(unansweredAtStart.filter(([key, write]) => refusedWriteSeq.get(key) !== write.seq)),
        ...writeValues(unansweredWrites),
      },
    };
    applyUiScale(previewedUiScale ?? currentUiScale());
    const t = currentThemeMode();
    if (t !== lastAppliedThemeMode) applyThemeMode(t);
    applyFontFamily(currentFontFamily());
    if (workspaceFontList.value.status === 'not-loaded') void loadWorkspaceFonts();
    currentModel.value = currentChatModel();
    notificationsFilter.value = currentNotificationsFilter();
    cacheServedValue('notification_toasts', NOTIFICATION_TOASTS_KEY);
    // The cache first: a stale mirror would otherwise answer for an unset key.
    cacheServedValue('autocorrect', AUTOCORRECT_STORAGE_KEY);
    setProseAutocorrect(currentAutocorrect());
    cacheServedValue('motion', MOTION_STORAGE_KEY);
    motionPreference.value = currentMotion();
    cacheServedValue('theme-effects', THEME_EFFECTS_STORAGE_KEY);
    themeEffectsPreference.value = currentThemeEffects();
    selectedCodingAgent.value = currentCodingAgentDefault();
    refreshActiveThemeIfChanged();
    // LAST, deliberately: the three applies above write properties the remote
    // is allowed to override, so the overrides go on top of them.
    applyStyleOverridesFromPreferences();
  } catch (e) {
    if (mySeq !== preferencesLoadSeq) return newestPreferencesLoad;
    // A failed REFETCH keeps the loaded preferences rather than blanking every
    // setting to its default. SSE only re-fires an already-`loaded` value, so a
    // flip to `failed` would never recover until a page reload. Only a first
    // load records the failure. Matches `loadWebhookIngress`.
    preferences.value = failedIfFresh(preferences.value, e);
  }
}

// --- Vertex AI region ---

export function currentVertexRegion(): string {
  return currentPreference('vertex_region');
}

export function setVertexRegion(region: string): Promise<void> {
  return savePreference('vertex_region', region);
}

// --- Local OpenAI-compatible provider base URL ---

// Ollama's OpenAI-compatible endpoint.
export const DEFAULT_LOCAL_BASE_URL = PREFERENCE_CATALOG.local_base_url.fallback;

export function currentLocalBaseUrl(): string {
  if (preferences.value.status !== 'loaded') return '';
  return preferences.value.data['local_base_url'] || '';
}

export function setLocalBaseUrl(url: string): Promise<void> {
  return savePreference('local_base_url', url.trim());
}

// --- OpenCode Free (keyless) ---

/** Whether the keyless OpenCode Free tier is on. Off by default, because
 *  turning it on sends prompts anonymously to a third-party relay. */
export function currentOpenCodeFreeEnabled(): boolean {
  return currentPreference('opencode_free_enabled') === 'true';
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

/** The master switches over the three System One providers on that page:
 *  TypeSafe (Jev), Cloudflare Workers AI (Clef and Clef-flash), and a custom
 *  endpoint.
 *
 *  Same key family and same absent-means-on rule as the six, and deliberately
 *  outside `SwitchableProvider`. A System One model answers typed questions
 *  rather than holding a conversation, so it has no `ProviderKind` and never
 *  appears in `/health.configured_providers` (ADR 0220). Nothing keyed on that
 *  union may therefore reach one, which is what keeps the type from admitting
 *  them.
 *
 *  Read by the engine's `llm::judgment::select` on every judgment, so a switch
 *  takes effect with no restart. Off sends every site picked on that provider
 *  back to its chat model, whatever the per-site `judgment_*` preference says. */
export const PROVIDER_ENABLED_TYPESAFE_KEY = PREF_PROVIDER_ENABLED_TYPESAFE.key;
export const PROVIDER_ENABLED_CLOUDFLARE_WORKERS_AI_KEY = PREF_PROVIDER_ENABLED_CLOUDFLARE_WORKERS_AI.key;
export const PROVIDER_ENABLED_SYSTEM_ONE_CUSTOM_KEY = PREF_PROVIDER_ENABLED_SYSTEM_ONE_CUSTOM.key;

/** The custom System One endpoint's full request URL and the model it is
 *  asked for. Read by the engine's `llm::judgment::endpoint`. */
export const SYSTEM_ONE_CUSTOM_URL_KEY = PREF_SYSTEM_ONE_CUSTOM_URL.key;
export const SYSTEM_ONE_CUSTOM_MODEL_KEY = PREF_SYSTEM_ONE_CUSTOM_MODEL.key;

/** Whether a stored switch value reads as an explicit off.
 *
 *  Wider than `providerSwitchedOff`'s own check just above, deliberately. The
 *  six are answered by `/health`, which already reflects the engine's parse, so
 *  a spelling only this side misreads is corrected there. A System One
 *  provider has no `/health` row (ADR 0220), which makes this the sole
 *  authority for its switch position: reading `off` as on would draw the row
 *  live while the engine ran chat. Absent is not off, because absent is the
 *  default. */
export function switchValueReadsAsOff(raw: string | undefined): boolean {
  return raw !== undefined && flagReading(raw) === 'false';
}

/** Switch one System One provider on or off, by its switch key. Off leaves
 *  the stored credential alone, exactly as the six do: the switch parks the
 *  provider, and Remove is what deletes the key. */
export function setSystemOneEnabled(switchKey: string, enabled: boolean): Promise<void> {
  return savePreference(switchKey, enabled ? 'true' : 'false');
}

// --- Capture context ---

/** Per-step ContextAssembled capture toggle. The debugging capture ships
 *  dark. */
export function currentCaptureContext(): boolean {
  return currentPreference('capture_context') === 'true';
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
export type ExternalLinkTarget = PreferenceValues<'external_link_target'>;

/** The default applies both when unset and while preferences are still
 *  loading. A link tapped during startup then can't fall into a different mode
 *  than the same link tapped a second later. An unrecognized stored value
 *  degrades to the default rather than disabling the hand-off. */
export function currentExternalLinkTarget(): ExternalLinkTarget {
  return currentPreference('external_link_target');
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

// --- Mobile dynamic bars ---

/** When true, the mobile header and prompt slide away on scroll down and come
 *  back on scroll up (`hooks/useHideOnScroll.ts`). When false, both stay
 *  pinned. */
export function currentMobileDynamicBars(): boolean {
  return currentPreference('mobile_dynamic_bars') === 'true';
}

export function setMobileDynamicBars(enabled: boolean): Promise<void> {
  return savePreference('mobile_dynamic_bars', enabled ? 'true' : 'false');
}

// --- Background model ---

/** Background model preference keys, stored in the DB and read by the engine.
 *  Each is half of a *model selection*; the paired `reasoning_*` key below
 *  carries the effort. The engine resolves the pair per `ContextPurpose` in
 *  `engine::aux_purpose`. */
export type BackgroundModelKey =
  | 'model_title'
  | 'model_change_summary'
  | 'model_image_description'
  | 'model_memory'
  | 'model_conversation_summary'
  | 'model_query_classification'
  | 'model_summary_compaction'
  | 'model_command_judge';

/** The reasoning half of each background *model selection*. */
export type BackgroundReasoningKey =
  | 'reasoning_title'
  | 'reasoning_change_summary'
  | 'reasoning_image_description'
  | 'reasoning_memory'
  | 'reasoning_conversation_summary'
  | 'reasoning_query_classification'
  | 'reasoning_summary_compaction'
  | 'reasoning_command_judge';

/** A background row's stored *model selection*, each half `null` while unset.
 *  The engine resolves an unset half, inheritance and the configured providers
 *  included, and `GET /api/v1/models/background` serves the result
 *  (`useBackgroundModels`). */
export function storedBackgroundSelection(
  modelKey: BackgroundModelKey,
  reasoningKey: BackgroundReasoningKey,
): { model: string | null; effort: string | null } {
  const data = preferences.value.status === 'loaded' ? preferences.value.data : {};
  return {
    model: acceptedValue(modelKey, data[modelKey]),
    effort: acceptedValue(reasoningKey, data[reasoningKey]),
  };
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

// --- Memory module ---

/** How a turn gets its past (ADR 0362). Global. */
export type MemoryModule = PreferenceValues<'memory_module'>;

export function currentMemoryModule(): MemoryModule {
  return currentPreference('memory_module');
}

export function setMemoryModule(module: MemoryModule): Promise<void> {
  return savePreference('memory_module', module);
}

// --- Compose destination (coding-agent chip + hand-off hint) ---

/** The account default coding agent — the SEED for a fresh compose's backend
 *  chip (via `selectedCodingAgent`, set at `loadPreferences`). Workspace-scoped
 *  (not device-scoped).
 *  Compose picks are per-draft (see `composeSelections`) and deliberately do NOT
 *  write this back (draft-only), so there is no `setCodingAgentDefault`. */
export function currentCodingAgentDefault(): CodingAgent {
  return currentPreference('coding_agent_default');
}

/** The Claude Code permission modes Lucidos offers. CC has six; the others
 *  are withheld deliberately (see the engine's `CcPermissionMode`). */
export const CC_PERMISSION_MODES = PREFERENCE_CATALOG.coding_agent_claude_permission_mode.values;
export type CcPermissionMode = PreferenceValues<'coding_agent_claude_permission_mode'>;

/** Which of Claude Code's own permission modes coding-agent threads run in.
 *  Workspace-scoped. */
export function currentCodingAgentPermissionMode(): CcPermissionMode {
  return currentPreference('coding_agent_claude_permission_mode');
}

export function setCodingAgentPermissionMode(mode: CcPermissionMode): Promise<void> {
  return savePreference('coding_agent_claude_permission_mode', mode);
}

// --- New-workspace welcome + starter suggestions ---

/** Whether the new-workspace welcome has been retired. The rule lives in the
 *  store's `welcomeDismissed`, which the compose layout also reads. */
export function welcomeSuggestionsDismissed(): boolean {
  return welcomeDismissed.value;
}

/** One-way retire: the user clicked "Don't show this again" on the
 *  new-workspace welcome, or `retireWelcomeAfterUse` decided they no longer
 *  need it. Idempotent: skips the write only when the LOADED preference
 *  already says dismissed. */
export function dismissWelcomeSuggestions(): Promise<void> {
  if (preferences.value.status === 'loaded'
    && preferences.value.data['welcome_suggestions_dismissed'] === 'true') {
    return Promise.resolve();
  }
  return savePreference('welcome_suggestions_dismissed', 'true');
}

/** How many threads a user starts before the welcome retires itself. */
export const WELCOME_RETIRES_AFTER_THREADS = 3;

/** Retires the welcome once the user has started enough threads to know the
 *  composer, so nobody carries it for months. Runs after each send and saves
 *  the same preference as "Don't show this again". It counts top-level threads
 *  the user sent: drafts, trigger runs, sub-threads and the home thread, which
 *  boot made, are not use of the composer. Desktop keeps the header's ? button as the way back into the
 *  setup interview. With no provider the welcome is the provider-setup call to
 *  action, which the same preference hides, so it never retires then. */
export function retireWelcomeAfterUse(threads: Iterable<ThreadState>): Promise<void> {
  if (!llmConfigured.value || welcomeSuggestionsDismissed()) return Promise.resolve();
  let started = 0;
  for (const { meta } of threads) {
    if (meta.initiator === 'user' && meta.state === 'active' && !meta.parentThreadId && !meta.home) started++;
  }
  return started >= WELCOME_RETIRES_AFTER_THREADS ? dismissWelcomeSuggestions() : Promise.resolve();
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

/** Master toggle for the command guard (the bash/python safety gate). The
 *  feature ships dark and is enabled per-workspace. */
export function currentCommandGuard(): boolean {
  return currentPreference('command_guard') === 'true';
}

export function setCommandGuard(enabled: boolean): Promise<void> {
  return savePreference('command_guard', enabled ? 'true' : 'false');
}

/** Sub-toggle for the LLM judge — when off, the guard uses only the static
 *  "dangerous" list for the ask lane. Only meaningful while the master
 *  `command_guard` toggle is on. */
export function currentCommandGuardJudge(): boolean {
  return currentPreference('command_guard_judge') === 'true';
}

export function setCommandGuardJudge(enabled: boolean): Promise<void> {
  return savePreference('command_guard_judge', enabled ? 'true' : 'false');
}

// --- Voice ---

/**
 * Whether this workspace has voice turned on at all.
 *
 * An unloaded preference set reads the catalog default, which is off. That is
 * the right way round: the call control appears once we know it should, never
 * in the gap before the answer arrives.
 */
export function voiceEnabled(): boolean {
  return currentPreference('voice_enabled') === 'true';
}

export function setVoiceEnabled(enabled: boolean): Promise<void> {
  return savePreference('voice_enabled', enabled ? 'true' : 'false');
}

/** The speech-to-speech model a *voice session* speaks through. Deliberately
 *  NOT a chat-model registry row, so it is a typed id rather than a pick from
 *  `backgroundModelChoices()`: a realtime model cannot serve an ordinary turn
 *  and never appears in that registry. */
export const DEFAULT_VOICE_TALKER_MODEL = PREFERENCE_CATALOG.model_voice_talker.fallback;

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

/** The talker model a call dials: the stored one, else the default. */
export function currentVoiceTalkerModel(): string {
  return currentPreference('model_voice_talker');
}

export function setVoiceTalkerModel(model: string): Promise<void> {
  return savePreference('model_voice_talker', model.trim());
}

/** The model that turns the caller's speech into text inside the talker's
 *  socket. The second and last model in the voice loop: nothing translates and
 *  nothing summarises. */
export const DEFAULT_VOICE_TRANSCRIBER_MODEL = PREFERENCE_CATALOG.model_voice_transcriber.fallback;

export function storedVoiceTranscriberModel(): string {
  if (preferences.value.status !== 'loaded') return '';
  return preferences.value.data['model_voice_transcriber'] ?? '';
}

export function currentVoiceTranscriberModel(): string {
  return currentPreference('model_voice_transcriber');
}

export function setVoiceTranscriberModel(model: string): Promise<void> {
  return savePreference('model_voice_transcriber', model.trim());
}

/** The voice a call is spoken in, as the provider's own name for one. Not a
 *  model, and not the language. */
export const DEFAULT_VOICE_TALKER_VOICE = PREFERENCE_CATALOG.voice_talker_voice.fallback;

export function storedVoiceTalkerVoice(): string {
  if (preferences.value.status !== 'loaded') return '';
  return preferences.value.data['voice_talker_voice'] ?? '';
}

export function currentVoiceTalkerVoice(): string {
  return currentPreference('voice_talker_voice');
}

export function setVoiceTalkerVoice(voice: string): Promise<void> {
  return savePreference('voice_talker_voice', voice.trim());
}

/**
 * The sections of the resident block, as the toggles need to draw them.
 *
 * What a call loads before it starts. The talker looks nothing up mid-call, so
 * this block is the whole of what it answers without waiting for the agent.
 *
 * Generated from `SECTIONS` in `crates/lucidos-engine/src/voice/sections.rs`,
 * which decides the ids and headings beside the builders that fill them. Which
 * ones ship on is the catalog default, {@link DEFAULT_VOICE_RESIDENT_SECTIONS}.
 */
export { VOICE_RESIDENT_SECTIONS };

/** The ids a call opens with while nothing is stored. */
export const DEFAULT_VOICE_RESIDENT_SECTIONS: readonly string[] =
  PREFERENCE_CATALOG.voice_resident_sections.fallback.split(',');

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
  if (selection === null) return DEFAULT_VOICE_RESIDENT_SECTIONS.includes(id);
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
  const current = new Set(voiceResidentSelection() ?? DEFAULT_VOICE_RESIDENT_SECTIONS);
  if (on) current.add(id);
  else current.delete(id);
  const registry: readonly string[] = VOICE_RESIDENT_SECTIONS.map((s) => s.id);
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
