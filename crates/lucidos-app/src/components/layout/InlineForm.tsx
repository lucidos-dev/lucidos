import { useEffect } from 'preact/hooks';
import { activeInlineForm, closeInlineForm } from '../../store/store';
import { CredentialModal } from '../credentials/CredentialModal';
import { AppUiEditModal } from '../apps/AppUiEditModal';
import { NewAppModal } from '../apps/NewAppModal';
import { TriggerDetails } from '../triggers/TriggerDetails';
import { EmailConfirmModal } from '../email/EmailConfirmModal';

export function InlineForm() {
  const form = activeInlineForm.value;

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape' && activeInlineForm.value) {
        closeInlineForm();
      }
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, []);

  if (!form) return null;

  switch (form.type) {
    case 'credential': return <CredentialModal />;
    case 'app-edit': return <AppUiEditModal />;
    case 'new-app': return <NewAppModal />;
    case 'trigger': return <TriggerDetails key={form.taskId ?? 'new'} />;
    case 'email-confirm': return <EmailConfirmModal />;
  }
}
