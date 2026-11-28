import { useState } from 'preact/hooks';
import type { Ref } from 'preact';
import { EyeIcon, EyeOffIcon } from './icons';

interface SecretInputProps {
  inputRef?: Ref<HTMLInputElement>;
  defaultValue?: string;
  placeholder?: string;
  required?: boolean;
}

export function SecretInput({ inputRef, defaultValue, placeholder, required }: SecretInputProps) {
  const [revealed, setRevealed] = useState(false);
  return (
    <div class="secret-input">
      <input
        ref={inputRef}
        type={revealed ? 'text' : 'password'}
        defaultValue={defaultValue}
        placeholder={placeholder}
        required={required}
        autocomplete="off"
        spellcheck={false}
      />
      <button
        type="button"
        class="secret-input-toggle"
        aria-label={revealed ? 'Hide secret' : 'Show secret'}
        data-tooltip={revealed ? 'Hide' : 'Show'}
        onClick={() => setRevealed((v) => !v)}
      >
        {revealed ? <EyeOffIcon /> : <EyeIcon />}
      </button>
    </div>
  );
}
