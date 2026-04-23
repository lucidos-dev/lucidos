import { activeInlineForm, closeInlineForm, showToast } from '../../store/store';
import { sendEmailConfirmed } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';

export function EmailConfirmModal() {
  const form = activeInlineForm.value;
  if (form?.type !== 'email-confirm') return null;

  const draft = form.request;

  async function handleSend() {
    try {
      const result = await sendEmailConfirmed({
        to: draft.to,
        subject: draft.subject,
        body: draft.body,
        cc: draft.cc,
        bcc: draft.bcc,
        reply_to_message_id: draft.reply_to_message_id,
        account: draft.account,
        attachments: draft.attachments,
      });
      if (result.success) {
        showToast('Email sent successfully', 'success');
      } else {
        showToast(result.error || 'Failed to send email', 'error');
      }
    } catch (e) {
      showToast('Failed to send email: ' + errorDetail(e), 'error');
    }
    closeInlineForm();
  }

  return (
    <div class="inline-form">
      <div style="padding: 0">
        <div class="form-group">
          <label>From</label>
          <div style="padding: 0.25rem 0; color: var(--text-primary)">{draft.from}</div>
        </div>
        <div class="form-group">
          <label>To</label>
          <div style="padding: 0.25rem 0; color: var(--text-primary)">{draft.to.join(', ')}</div>
        </div>
        {draft.cc && draft.cc.length > 0 && (
          <div class="form-group">
            <label>CC</label>
            <div style="padding: 0.25rem 0; color: var(--text-primary)">{draft.cc.join(', ')}</div>
          </div>
        )}
        <div class="form-group">
          <label>Subject</label>
          <div style="padding: 0.25rem 0; color: var(--text-primary); font-weight: 500">{draft.subject}</div>
        </div>
        <div class="form-group">
          <label>Body</label>
          <pre style="padding: 0.5rem; background: var(--bg-secondary); border-radius: 0.25rem; white-space: pre-wrap; font-size: 0.85rem; max-height: 18.75rem; overflow-y: auto; color: var(--text-primary)">{draft.body}</pre>
        </div>
        {draft.attachment_names && draft.attachment_names.length > 0 && (
          <div class="form-group">
            <label>Attachments</label>
            <div style="padding: 0.25rem 0; color: var(--text-primary)">{draft.attachment_names.join(', ')}</div>
          </div>
        )}
        <div class="form-actions">
          <button type="button" class="btn-cancel" onClick={() => closeInlineForm()}>Cancel</button>
          <button type="button" class="btn-save" onClick={handleSend}>Send Email</button>
        </div>
      </div>
    </div>
  );
}
