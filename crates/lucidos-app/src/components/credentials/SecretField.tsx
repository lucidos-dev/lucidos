import { useState } from 'preact/hooks';
import type { ComponentChildren, Ref } from 'preact';
import { SecretInput } from '../shared/SecretInput';
import { isSecretReference } from './credentialSecret';

interface SecretFieldProps {
  label: ComponentChildren;
  inputRef: Ref<HTMLInputElement>;
  initialValue: string;
  placeholder: string;
  required?: boolean;
}

export function SecretField({ label, inputRef, initialValue, placeholder, required }: SecretFieldProps) {
  const storedReference = isSecretReference(initialValue);
  const [fromOnePassword, setFromOnePassword] = useState(storedReference);
  const initialForSource = storedReference === fromOnePassword ? initialValue : '';
  return (
    <div class="form-group">
      <div class="secret-field-label-row">
        {label}
        <label class="secret-field-source">
          From 1Password
          <span class="toggle-switch">
            <input
              type="checkbox"
              checked={fromOnePassword}
              onChange={(e) => setFromOnePassword((e.currentTarget as HTMLInputElement).checked)}
            />
            <span class="toggle-slider" />
          </span>
        </label>
      </div>
      {fromOnePassword ? (
        <>
          <input
            key="reference"
            ref={inputRef}
            type="text"
            defaultValue={initialForSource}
            placeholder="op://vault/item/field"
            required={required}
            spellcheck={false}
          />
          <p class="secret-field-hint">Read from 1Password each time it is used. Never stored here.</p>
        </>
      ) : (
        <SecretInput
          key="value"
          inputRef={inputRef}
          defaultValue={initialForSource}
          placeholder={placeholder}
          required={required}
        />
      )}
    </div>
  );
}
