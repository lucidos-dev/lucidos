import { useState } from 'preact/hooks';
import { currentCaptureContext, setCaptureContext } from '../../store/actions/preferences';
import { isPerfEnabled, setPerfEnabled } from '../../utils/perfQueue';
import { animationSpeed, speedMultiplier } from '../../store/store';

/** Settings → System → Debugging: developer/diagnostic toggles, off by default.
 *
 *  - "Capture context per step" is a server preference (`capture_context`),
 *    moved here from the former Models → Debugging group so all diagnostics share
 *    one home. Calling `currentCaptureContext()` in render subscribes to
 *    preference updates (it reads `preferences.value`), same as LocaleSection.
 *  - "Perf instrumentation" toggles the per-device `lucidos:perf` localStorage
 *    flag that gates `recordPerfSample` (see utils/perfQueue.ts). It's NOT a
 *    server preference — device-local by design, read live, takes effect on the
 *    next sample with no reload. localStorage isn't reactive, so the checked
 *    state is held locally for immediate UI feedback; the gate reads the flag
 *    directly.
 *  - "Animation speed" scales every UI transition via `speedMultiplier` (a
 *    device-global slider persisted in localStorage, see store.ts/effects.ts).
 *    It lives here as a diagnostic — slowing animations down makes transition
 *    glitches inspectable; reading `animationSpeed.value` in render subscribes
 *    to the signal so the multiplier label updates live as you drag. */
export function DebuggingSection() {
  // Lazy initializer: read localStorage once on mount, not on every render. The
  // panel remounts each time it's opened, so this reflects the current flag.
  const [perfOn, setPerfOn] = useState(() => isPerfEnabled());

  return (
    <div class="settings-section">
      <div class="settings-section-title">Debugging</div>
      <div class="settings-row" data-search-anchor="debugging:capture-context">
        <span class="settings-row-label">Capture context per step</span>
        <label class="toggle-switch">
          <input
            type="checkbox"
            checked={currentCaptureContext()}
            onChange={(e) => void setCaptureContext((e.currentTarget as HTMLInputElement).checked)}
          />
          <span class="toggle-slider" />
        </label>
      </div>
      <div class="settings-row-note">
        Stores the text of each prompt section (system prompt, conversation,
        tools, memory, …) sent to the model on every step, so you can inspect what
        went into the context in a step's snapshot viewer. Off by default — it
        enlarges the event log; while off, the snapshot still shows each section's
        name and size, just not its contents.
      </div>
      <div class="settings-row" data-search-anchor="debugging:perf">
        <span class="settings-row-label">Perf instrumentation</span>
        <label class="toggle-switch">
          <input
            type="checkbox"
            checked={perfOn}
            onChange={(e) => {
              const on = (e.currentTarget as HTMLInputElement).checked;
              setPerfEnabled(on);
              setPerfOn(on);
            }}
          />
          <span class="toggle-slider" />
        </label>
      </div>
      <div class="settings-row-note">
        Logs thread-open / render / linkify timings to the engine log
        (<code>[Client/perf]</code> lines) for diagnosing lag. Per-device and off
        by default — turn it on only while measuring. Takes effect immediately, no
        reload.
      </div>
      <div class="settings-row" data-search-anchor="debugging:animation-speed">
        <span class="settings-row-label">Animation speed</span>
        <div class="settings-row-options" style="gap: 0.5rem; align-items: center">
          <input
            type="range"
            min="-10"
            max="10"
            step="1"
            value={animationSpeed.value}
            onInput={(e) => {
              animationSpeed.value = parseInt((e.target as HTMLInputElement).value);
            }}
            style="width: 7rem"
          />
          <span class="settings-row-label" style="min-width: 2.5rem; text-align: right">{speedMultiplier.value.toFixed(1)}x</span>
        </div>
      </div>
      <div class="settings-row-note">
        Scales the duration of every UI transition (1.0x = normal). Slowing
        animations down makes transition glitches easier to inspect. Per-device,
        persisted locally, takes effect immediately.
      </div>
    </div>
  );
}
