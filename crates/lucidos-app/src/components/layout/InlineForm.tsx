import { useEffect } from 'preact/hooks';
import { activeInlineForm, closeInlineForm } from '../../store/store';
import { lazyComponent } from '../../utils/lazyComponent';

const CredentialModal = lazyComponent(() => import('../credentials/CredentialModal').then(m => m.CredentialModal));
const AppUiEditModal = lazyComponent(() => import('../apps/AppUiEditModal').then(m => m.AppUiEditModal));
const NewAppModal = lazyComponent(() => import('../apps/NewAppModal').then(m => m.NewAppModal));
const TriggerDetails = lazyComponent(() => import('../triggers/TriggerDetails').then(m => m.TriggerDetails));
const EmailConfirmModal = lazyComponent(() => import('../email/EmailConfirmModal').then(m => m.EmailConfirmModal));
const PluginInstallPanel = lazyComponent(() => import('../plugins/PluginInstallPanel').then(m => m.PluginInstallPanel));
const PluginUninstallPanel = lazyComponent(() => import('../plugins/PluginUninstallPanel').then(m => m.PluginUninstallPanel));

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
    case 'trigger': return <TriggerDetails key={form.triggerId ?? 'new'} />;
    case 'email-confirm': return <EmailConfirmModal />;
    case 'plugin-install': return <PluginInstallPanel />;
    case 'plugin-uninstall': return <PluginUninstallPanel />;
  }
}
