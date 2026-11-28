import type { JSX } from 'preact';

export function HiddenFileInput(props: {
  accept?: string;
  multiple?: boolean;
  onChange: (e: Event) => void;
  name?: string;
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

function openOnEnter(e: KeyboardEvent): void {
  if (e.key !== 'Enter') return;
  e.preventDefault();
  (e.currentTarget as HTMLInputElement).click();
}
