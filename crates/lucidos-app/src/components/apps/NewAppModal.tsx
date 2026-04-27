import { useRef, useEffect, useState } from 'preact/hooks';
import { closeInlineForm } from '../../store/store';
import { submitNewApp } from '../../store/actions/apps';
import { AutoTextarea } from '../shared/AutoTextarea';

export function NewAppModal() {
  const nameRef = useRef<HTMLInputElement>(null);
  const [description, setDescription] = useState('');

  useEffect(() => {
    nameRef.current?.focus();
  }, []);

  function handleSubmit(e: Event) {
    e.preventDefault();
    const name = nameRef.current?.value.trim();
    if (!name) return;
    submitNewApp(name, description.trim());
  }

  return (
    <div class="inline-form">
      <form onSubmit={handleSubmit}>
        <div class="form-group">
          <label>Name</label>
          <input
            ref={nameRef}
            type="text"
            placeholder="e.g. habit-tracker"
            required
          />
        </div>
        <div class="form-group">
          <label>Description</label>
          <AutoTextarea
            value={description}
            onInput={setDescription}
            placeholder="What should this app do?"
          />
        </div>
        <div class="form-actions">
          <button type="button" class="btn-cancel" onClick={() => closeInlineForm()}>
            Cancel
          </button>
          <button type="submit" class="btn-save">
            Create
          </button>
        </div>
      </form>
    </div>
  );
}
