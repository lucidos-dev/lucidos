import { useRef, useState } from 'preact/hooks';
import { activeInlineForm, closeInlineForm, showToast } from '../../store/store';
import type { EmailConfirmForm } from '../../store/store';
import { cancelEmailConfirm, markEmailSent } from '../../store/actions/email-confirm';
import { sendEmailConfirmed } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { AutoTextarea } from '../shared/AutoTextarea';
import { PROSE_TEXT_ATTRS } from '../../utils/noAutofill';

export async function driveSend(io: {
  isSending: () => boolean;
  setSending: (v: boolean) => void;
  send: () => Promise<{ success: boolean; error?: string }>;
  toast: (msg: string, type: 'success' | 'error') => void;
  markSent: () => boolean;
}): Promise<void> {
  if (io.isSending()) return;
  io.setSending(true);
  try {
    const result = await io.send();
    if (result.success) {
      if (!io.markSent()) io.toast('Email sent successfully', 'success');
    } else {
      io.toast(result.error || 'Failed to send email', 'error');
    }
  } catch (err) {
    io.toast('Failed to send email: ' + errorDetail(err), 'error');
  } finally {
    io.setSending(false);
  }
}

export function EmailConfirmModal() {
  const form = activeInlineForm.value;
  if (form?.type !== 'email-confirm') return null;
  return form.sentAt
    ? <EmailSentReceipt sent={form.request} sentAt={form.sentAt} />
    : <EmailConfirmDraft form={form} />;
}

function EmailMeta({ request, subject }: { request: EmailConfirmForm['request']; subject?: string }) {
  return (
    <div class="email-confirm-meta">
      <div class="email-confirm-meta-row">
        <span class="email-confirm-meta-label">From</span>
        <span class="email-confirm-meta-value">{request.from}</span>
      </div>
      <div class="email-confirm-meta-row">
        <span class="email-confirm-meta-label">To</span>
        <span class="email-confirm-meta-value">{request.to.join(', ')}</span>
      </div>
      {request.cc && request.cc.length > 0 && (
        <div class="email-confirm-meta-row">
          <span class="email-confirm-meta-label">CC</span>
          <span class="email-confirm-meta-value">{request.cc.join(', ')}</span>
        </div>
      )}
      {request.bcc && request.bcc.length > 0 && (
        <div class="email-confirm-meta-row">
          <span class="email-confirm-meta-label">BCC</span>
          <span class="email-confirm-meta-value">{request.bcc.join(', ')}</span>
        </div>
      )}
      {subject != null && (
        <div class="email-confirm-meta-row">
          <span class="email-confirm-meta-label">Subject</span>
          <span class="email-confirm-meta-value">{subject}</span>
        </div>
      )}
    </div>
  );
}

function EmailAttachments({ names }: { names: string[] }) {
  return (
    <div class="form-group">
      <label>Attachments</label>
      <div class="email-confirm-attachments">
        {names.map((name) => (
          <span class="email-confirm-attachment" key={name}>{name}</span>
        ))}
      </div>
    </div>
  );
}

function EmailConfirmDraft({ form }: { form: EmailConfirmForm }) {
  const draft = form.request;
  const [subject, setSubject] = useState(draft.subject);
  const [body, setBody] = useState(draft.body);
  const sendingRef = useRef(false);
  const [sending, setSending] = useState(false);

  const handleSend = () => {
    void driveSend({
      isSending: () => sendingRef.current,
      setSending: (v: boolean) => { sendingRef.current = v; setSending(v); },
      send: () => {
        return sendEmailConfirmed({
          to: draft.to,
          subject,
          body,
          cc: draft.cc,
          bcc: draft.bcc,
          reply_to_message_id: draft.reply_to_message_id,
          account: draft.account,
          attachments: draft.attachments,
          form_request_id: draft.form_request_id,
        });
      },
      toast: showToast,
      markSent: () => markEmailSent(form, { subject, body }),
    });
  };

  return (
    <div class="inline-form email-confirm protected-surface">
      <EmailMeta request={draft} />
      <div class="form-group">
        <label>Subject</label>
        <input
          type="text"
          value={subject}
          onInput={(e) => setSubject((e.target as HTMLInputElement).value)}
          {...PROSE_TEXT_ATTRS}
        />
      </div>
      <div class="form-group">
        <label>Body</label>
        <AutoTextarea value={body} onInput={setBody} />
      </div>
      {draft.attachment_names && draft.attachment_names.length > 0 && (
        <EmailAttachments names={draft.attachment_names} />
      )}
      <div class="form-actions">
        <button type="button" class="btn-cancel" onClick={() => cancelEmailConfirm(form)} disabled={sending}>Cancel</button>
        <button type="button" class="btn-save" onClick={handleSend} disabled={sending}>{sending ? 'Sending…' : 'Send Email'}</button>
      </div>
    </div>
  );
}

function EmailSentReceipt({ sent, sentAt }: { sent: EmailConfirmForm['request']; sentAt: string }) {
  return (
    <div class="inline-form email-confirm protected-surface">
      <div class="panel-receipt-status">
        <span class="panel-receipt-badge">Sent</span>
        <span class="panel-receipt-time">{formatMessageTimestamp(sentAt)}</span>
      </div>
      <EmailMeta request={sent} subject={sent.subject} />
      <div class="form-group">
        <label>Body</label>
        <div class="email-confirm-body-sent">{sent.body}</div>
      </div>
      {sent.attachment_names && sent.attachment_names.length > 0 && (
        <EmailAttachments names={sent.attachment_names} />
      )}
      <div class="form-actions">
        <button type="button" class="btn-cancel" onClick={() => closeInlineForm()}>Close</button>
      </div>
    </div>
  );
}
