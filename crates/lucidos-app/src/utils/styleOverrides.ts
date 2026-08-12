/**
 * The live style remote: the app-side half.
 *
 * The VALIDATOR and its caps live in the shared appearance contract
 * (`@lucidos/appearance`), re-exported below so this module stays the one
 * import site for app code. They moved there when the two FOUC scripts were
 * collapsed into one generated boot script: that script applies the same map
 * before any module can run, and it must not reach a different verdict than
 * this module does a moment later. There is now one copy of the rules, and
 * `appearance.test.ts` owns them.
 *
 * A `style_overrides` preference holds a JSON object of custom property name to
 * value. It is applied straight onto `<html>` by `applyStyleOverrides`
 * (store/actions/preferences.ts), which is what lets a design value be retuned
 * on a running app with no rebuild and no Apply: preferences already fan out
 * live over the `PreferencesChanged` SSE.
 */

export {
  STYLE_OVERRIDES_STORAGE_KEY,
  STYLE_RESET_PARAM,
  MAX_STYLE_OVERRIDES,
  MAX_STYLE_VALUE_LENGTH,
  isValidOverrideName,
  isValidOverrideValue,
  parseStyleOverrides,
  styleResetRequested,
} from '@lucidos/appearance';

/** Preference key holding the serialized map. */
export const STYLE_OVERRIDES_KEY = 'style_overrides';

export function serializeStyleOverrides(map: Record<string, string>): string {
  return JSON.stringify(map);
}

