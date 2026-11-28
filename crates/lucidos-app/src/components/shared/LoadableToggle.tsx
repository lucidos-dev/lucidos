export function LoadableToggle(props: {
  loaded: boolean;
  checked: boolean;
  disabled?: boolean;
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
