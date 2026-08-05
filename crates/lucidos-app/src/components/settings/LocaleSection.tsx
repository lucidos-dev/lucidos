import { Dropdown } from '../shared/Dropdown';
import type { DropdownOption } from '../shared/Dropdown';
import {
  currentLanguage,
  setLanguage,
  currentTimezone,
  setTimezone,
} from '../../store/actions/preferences';

/** Common languages offered up-front; the field is freeText, so any other
 *  language name can still be typed. The value stored is the plain name the
 *  engine echoes back ("Respond in {language}"). */
const COMMON_LANGUAGES = [
  'English',
  'Norwegian',
  'Swedish',
  'Danish',
  'German',
  'French',
  'Spanish',
  'Italian',
  'Portuguese',
  'Dutch',
  'Polish',
  'Finnish',
  'Japanese',
  'Chinese',
  'Korean',
];

/** All IANA timezone names from the platform (one source of truth), falling back
 *  to a small common list on the rare engine without `Intl.supportedValuesOf`. */
function timezoneOptions(current: string): DropdownOption[] {
  let zones: string[];
  try {
    const supported = (
      Intl as unknown as { supportedValuesOf?: (k: string) => string[] }
    ).supportedValuesOf;
    zones = supported ? supported('timeZone') : [];
  } catch {
    zones = [];
  }
  if (zones.length === 0) {
    zones = [
      'UTC',
      'Europe/Oslo',
      'Europe/London',
      'America/New_York',
      'America/Chicago',
      'America/Denver',
      'America/Los_Angeles',
      'Asia/Tokyo',
      'Asia/Shanghai',
      'Asia/Kolkata',
      'Australia/Sydney',
    ];
  }
  // Make sure the saved value is always selectable, even if the platform list
  // omits it (stale alias, custom zone).
  if (current && !zones.includes(current)) zones = [current, ...zones];
  return zones.map((z) => ({ value: z, label: z }));
}

/** Language + timezone: global (workspace-wide) preferences, surfaced as their
 *  own Settings category so they're not LLM-only (the agent sets them via
 *  set_preference; this is the human-facing control). They rendered under
 *  Settings → System until 2026-08-05, which buried two of the most
 *  looked-for settings there are under a diagnostics page. */
export function LocaleSection() {
  const language = currentLanguage();
  const timezone = currentTimezone();

  const languageOptions: DropdownOption[] = COMMON_LANGUAGES.map((l) => ({
    value: l,
    label: l,
  }));

  return (
    <div class="settings-section">
      <div class="settings-section-title">Locale</div>
      <div class="settings-row" data-search-anchor="locale:language">
        <span class="settings-row-label">Language</span>
        <Dropdown
          options={languageOptions}
          value={language}
          onChange={(v) => void setLanguage(v)}
          freeText
          placeholder="Auto (from conversation)"
        />
      </div>
      <div class="settings-row-note">
        For best results, English is recommended — models are strongest in it. You
        can still write in any language; replies come in the language set here, or
        match yours when left on Auto.
      </div>
      <div class="settings-row" data-search-anchor="locale:timezone">
        <span class="settings-row-label">Timezone</span>
        <Dropdown
          options={timezoneOptions(timezone)}
          value={timezone}
          onChange={(v) => void setTimezone(v)}
          placeholder="Select a timezone"
        />
      </div>
    </div>
  );
}
