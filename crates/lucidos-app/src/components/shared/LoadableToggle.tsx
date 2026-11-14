/** A toggle switch backed by a `Loadable` value.
 *
 *  Until it has loaded the toggle renders a neutral placeholder pill, never a
 *  definite on/off position. The persisted value then mounts in its final spot,
 *  rather than animating across from the loading default on every page reload.
 *  The `.toggle-slider` knob has a 0.2s transition that would otherwise visibly
 *  slide off to on.
 *
 *  CSS transitions do not fire on initial mount, so a freshly-mounted checked
 *  toggle lands silently. The placeholder is a `<span>` rather than the real
 *  `<label>`, so Preact replaces the whole subtree when `loaded` flips. That
 *  guarantees the fresh mount, where an in-place `checked` update would
 *  animate. */
export function LoadableToggle(props: {
  loaded: boolean;
  checked: boolean;
  disabled?: boolean;
  /** Accessible name. Required wherever the visible label beside the switch is
   *  not a `<label>` bound to it, which is every row pairing two switches. */
  ariaLabel?: string;
  onChange: (checked: boolean) => void;
}) {
  if (!props.loaded) {
    return <span class="toggle-switch toggle-switch-loading" aria-hidden="true" />;
  }
  return (
    <label class={`toggle-switch${props.disabled ? ' toggle-switch-disabled' : ''}`}>
      <input
        type="checkbox"
        checked={props.checked}
        disabled={props.disabled}
        aria-label={props.ariaLabel}
        onChange={(e) => props.onChange((e.currentTarget as HTMLInputElement).checked)}
      />
      <span class="toggle-slider" />
    </label>
  );
}
