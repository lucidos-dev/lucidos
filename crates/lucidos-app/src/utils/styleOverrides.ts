
export {
  STYLE_OVERRIDES_STORAGE_KEY,
  STYLE_RESET_PARAM,
  MAX_STYLE_OVERRIDES,
  MAX_STYLE_VALUE_LENGTH,
  isAllowedOverride,
  isReservedOverrideName,
  isValidOverrideName,
  isValidOverrideValue,
  parseStyleOverrides,
  styleResetRequested,
} from '@lucidos/appearance';

export const STYLE_OVERRIDES_KEY = 'style_overrides';

export function serializeStyleOverrides(map: Record<string, string>): string {
  return JSON.stringify(map);
}

