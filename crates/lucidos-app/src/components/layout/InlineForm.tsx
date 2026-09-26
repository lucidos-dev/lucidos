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

/** Escape closes the open form, unless an earlier handler already spent it.
 *  The central dispatcher blurs a focused input with `preventDefault` but lets
 *  the event bubble, so one Escape must not also drop the form's unsaved edits. */
export function closeInlineFormOnEscape(e: Pick<KeyboardEvent, 'key' | 'defaultPrevented'>): void {
  if (e.key === 'Escape' && !e.defaultPrevented && activeInlineForm.value) {
    closeInlineForm();
  }
}

export function InlineForm() {
  const form = activeInlineForm.value;

  useEffect(() => {
    document.addEventListener('keydown', closeInlineFormOnEscape);
    return () => document.removeEventListener('keydown', closeInlineFormOnEscape);
  }, []);

  if (!form) return null;

  switch (form.type) {
    case 'credential': return <CredentialModal />;
    case 'app-edit': return <AppUiEditModal />;
    case 'new-app': return <NewAppModal />;
    case 'trigger': return <TriggerDetails key={form.triggerId ?? 'new'} />;
    // The draft editor seeds subject/body useState once per mount — the key
    // remounts it when a different draft replaces the open form (no request id
    // exists, so the state-seeding fields are the identity; JSON keeps field
    // boundaries unambiguous). The draft→receipt swap needs no key change: they
    // are different components, so flipping `sentAt` remounts on its own.
    case 'email-confirm':
      return (
        <EmailConfirmModal
          key={JSON.stringify([form.request.account, form.request.to, form.request.subject, form.request.body])}
        />
      );
    case 'plugin-install': return <PluginInstallPanel />;
    case 'plugin-uninstall': return <PluginUninstallPanel />;
  }
}
