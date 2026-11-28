import { useAutoResizeTextarea } from '../../hooks/useAutoResizeTextarea';
import { PROSE_TEXT_ATTRS } from '../../utils/noAutofill';

interface Props {
  value: string;
  onInput: (value: string) => void;
  placeholder?: string;
  required?: boolean;
}

export function AutoTextarea({ value, onInput, placeholder, required }: Props) {
  const ref = useAutoResizeTextarea(value);

  function handleInput(e: Event) {
    onInput((e.target as HTMLTextAreaElement).value);
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      const form = ref.current?.form;
      if (form) {
        e.preventDefault();
        form.requestSubmit();
      }
    }
  }

  return (
    <textarea
      ref={ref}
      class="auto-textarea"
      value={value}
      onInput={handleInput}
      onKeyDown={handleKeyDown}
      placeholder={placeholder}
      required={required}
      rows={1}
      {...PROSE_TEXT_ATTRS}
    />
  );
}
