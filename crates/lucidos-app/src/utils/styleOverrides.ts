/**
 * The live style remote: validation for user-supplied CSS custom property
 * overrides.
 *
 * A `style_overrides` preference holds a JSON object of custom property name to
 * value. It is applied straight onto `<html>` by `applyStyleOverrides`
 * (store/actions/preferences.ts), which is what lets a design value be retuned
 * on a running app with no rebuild and no Apply: preferences already fan out
 * live over the `PreferencesChanged` SSE.
 *
 * That reach is exactly why this file exists. Preferences are writable by any
 * app through `lucidos.preferences.set` and by the chat agent over the HTTP
 * API, so the map is an UNTRUSTED input path into the host's own inline style.
 * Everything that reaches `setProperty` passes through here first.
 *
 * The validator is mirrored in two other realms that apply the same map before
 * this module can run, and all three must agree:
 *   - `crates/lucidos-app/index.html`, the FOUC script, so a tuned value paints
 *     on the first frame instead of flashing the untuned one.
 *   - `crates/lucidos-engine/src/api/sdk_prefs.rs`, so app iframes match the
 *     shell (the same reason theme, font and scale are mirrored there).
 * `styleOverrides.test.ts` scans both mirrors for the same literals.
 */

/** Preference key holding the serialized map. */
export const STYLE_OVERRIDES_KEY = 'style_overrides';

/** Workspace-scoped localStorage key for the FOUC seed. Scoping is automatic in
 *  the app realm (`workspaceStorage.ts` overrides `Storage.prototype`); the
 *  boot script wraps it in `wsKey()` by hand. */
export const STYLE_OVERRIDES_STORAGE_KEY = 'lucidos-style-overrides';

/** URL parameter that clears the map before first paint. The escape hatch from
 *  a value that made the UI unusable, so it must not depend on any of the UI
 *  being legible. */
export const STYLE_RESET_PARAM = 'style-reset';

/** Cap on entries. A design remote tunes tens of values; a map in the thousands
 *  is a runaway writer, not a user. */
export const MAX_STYLE_OVERRIDES = 200;

/** Cap on one value's length. The longest real token in `base.css` is a layered
 *  box-shadow at 94 characters. */
export const MAX_STYLE_VALUE_LENGTH = 120;

/** Only a custom property can be set: never `color`, never a selector. */
const NAME_RE = /^--[a-z][a-z0-9-]*$/;

/**
 * Rejected value shapes, and why each one is here:
 *   `;`        closes the declaration, so the rest of the value becomes a
 *              SECOND declaration. The whole injection vector in one character.
 *   `{` `}`    closes the rule and opens a new selector block.
 *   `<` `>`    `</style>` breaks out of an inlined stylesheet context.
 *   `@`        `@import` pulls in a remote stylesheet.
 *   `\`        CSS escapes (`\3b`) spell any of the above past a naive scan.
 *   `url(`     issues a request to an origin the value's author chose, which
 *              leaks the fact of a page view to them. `image-set(` is the same
 *              hazard by another name.
 *   `expression(` legacy IE dynamic properties, still parsed by some engines.
 *   `/*`       opens a comment that swallows the rest of the block.
 * `var(`, `color-mix(`, `rgba(`, `calc(` and the gradient functions are all
 * fine and deliberately allowed: they are what the real tokens are made of.
 */
const VALUE_BANNED_RE = /[;{}<>@\\]|url\s*\(|image-set\s*\(|expression\s*\(|\/\*/i;

export function isValidOverrideName(name: string): boolean {
  return NAME_RE.test(name);
}

export function isValidOverrideValue(value: string): boolean {
  if (typeof value !== 'string') return false;
  const trimmed = value.trim();
  if (trimmed === '') return false;
  if (trimmed.length > MAX_STYLE_VALUE_LENGTH) return false;
  return !VALUE_BANNED_RE.test(trimmed);
}

/**
 * Parse the stored preference into a map safe to hand to `setProperty`.
 *
 * Invalid entries are DROPPED rather than failing the whole map: one bad value
 * written by an app must not cost the user every value they tuned by hand.
 */
export function parseStyleOverrides(raw: string | null | undefined): Record<string, string> {
  if (!raw) return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return {};
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return {};

  const out: Record<string, string> = {};
  let n = 0;
  for (const [name, value] of Object.entries(parsed as Record<string, unknown>)) {
    if (n >= MAX_STYLE_OVERRIDES) break;
    if (!isValidOverrideName(name)) continue;
    if (typeof value !== 'string' || !isValidOverrideValue(value)) continue;
    out[name] = value.trim();
    n++;
  }
  return out;
}

export function serializeStyleOverrides(map: Record<string, string>): string {
  return JSON.stringify(map);
}

/** Whether the current URL asks for the overrides to be dropped. */
export function styleResetRequested(search: string): boolean {
  return new RegExp(`[?&]${STYLE_RESET_PARAM}(?:[=&]|$)`).test(search);
}
