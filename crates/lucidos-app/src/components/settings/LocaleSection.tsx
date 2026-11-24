import { Dropdown } from '../shared/Dropdown';
import type { DropdownOption } from '../shared/Dropdown';
import {
  currentLanguage,
  setLanguage,
  currentTimezone,
  setTimezone,
} from '../../store/actions/preferences';
import { Explainer } from '../shared/Explainer';

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
  if (current && !zones.includes(current)) zones = [current, ...zones];
  return zones.map((z) => ({ value: z, label: z }));
}

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
        <span class="settings-row-label">
          Language
          <Explainer title="Language">
            <p>
              For best results, English is recommended: models are strongest in it.
            </p>
            <p>
              You can still write in any language; replies come in the language set
              here, or match yours when left on Auto.
            </p>
            <p>
              A voice call speaks this language too, and listens for it. Leave it
              on Auto and a call guesses per sentence, which is how Norwegian
              drifts between Bokmål and Nynorsk.
            </p>
          </Explainer>
        </span>
        <Dropdown
          options={languageOptions}
          value={language}
          onChange={(v) => void setLanguage(v)}
          freeText
          placeholder="Auto (from conversation)"
        />
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
