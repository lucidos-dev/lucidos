import type { JSX } from 'preact';

/**
 * A hidden `<input type="file">` that must be rendered INSIDE a `<label>`.
 *
 * The label-wrapper pattern is required for iOS PWA reliability. Programmatic
 * `.click()` on a hidden file input — especially from a touch handler that
 * also moved focus — is unreliable in standalone PWA mode: the picker opens,
 * the user selects a photo, but the `change` event after dismissal never
 * lands. With a label wrapping the input, the browser routes the tap straight
 * to the input as one uninterrupted native gesture.
 *
 * Hides via `.visually-hidden` (off-screen but still in layout). `display:none`
 * and `visibility:hidden` also drop the change event on iOS PWA.
 */
export function HiddenFileInput(props: {
  accept?: string;
  multiple?: boolean;
  onChange: (e: Event) => void;
}): JSX.Element {
  return (
    <input
      type="file"
      accept={props.accept}
      multiple={props.multiple}
      class="visually-hidden"
      tabIndex={-1}
      aria-hidden="true"
      onChange={props.onChange}
    />
  );
}
