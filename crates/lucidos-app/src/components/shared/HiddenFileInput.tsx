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
 *
 * Given a `name`, the input stays in the tab order as the keyboard route to
 * the picker. The wrapping label then draws its focus ring. Without a `name`
 * it is pointer-only.
 */
export function HiddenFileInput(props: {
  accept?: string;
  multiple?: boolean;
  onChange: (e: Event) => void;
  /** Accessible name; makes the input focusable. */
  name?: string;
  /** Id of the element that reads out the current pick. */
  describedBy?: string;
}): JSX.Element {
  const focusable = props.name !== undefined;
  return (
    <input
      type="file"
      accept={props.accept}
      multiple={props.multiple}
      class="visually-hidden"
      tabIndex={focusable ? undefined : -1}
      aria-hidden={focusable ? undefined : 'true'}
      aria-label={props.name}
      aria-describedby={props.describedBy}
      onKeyDown={focusable ? openOnEnter : undefined}
      onChange={props.onChange}
    />
  );
}

/** Space opens a file input in every engine; Enter does not in all of them. */
function openOnEnter(e: KeyboardEvent): void {
  if (e.key !== 'Enter') return;
  e.preventDefault();
  (e.currentTarget as HTMLInputElement).click();
}
