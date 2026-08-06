import { useRef, useState } from 'preact/hooks';
import { activeInlineForm, closeInlineForm, showToast } from '../../store/store';
import type { EmailConfirmForm } from '../../store/store';
import { markEmailSent } from '../../store/actions/email-confirm';
import { sendEmailConfirmed } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { AutoTextarea } from '../shared/AutoTextarea';
import { PROSE_TEXT_ATTRS } from '../../utils/noAutofill';

/** Send-flow driver, extracted hook-free for tests. Guards double-submit (an
 *  in-flight send makes further clicks a no-op — repeated clicks used to queue
 *  duplicate sends), toggles the pending flag around the request, and maps the
 *  three outcomes: success → the panel becomes a sent receipt in place (falling
 *  back to a success toast when the panel is gone, see `markSent`); engine
 *  `{success:false}` → error toast, form stays open for retry; thrown error →
 *  same via `errorDetail`. */
export async function driveSend(io: {
  isSending: () => boolean;
  setSending: (v: boolean) => void;
  send: () => Promise<{ success: boolean; error?: string }>;
  toast: (msg: string, type: 'success' | 'error') => void;
  /** Flip the open panel to its read-only receipt. Returns false when the panel
   *  is no longer the active overlay (the user dismissed it mid-send), in which
   *  case the toast is the only place the success can land. */
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
  // Two components rather than one branching on `sentAt`, so the draft's hooks
  // are never conditionally skipped: flipping to sent unmounts the editor and
  // mounts the receipt.
  return form.sentAt
    ? <EmailSentReceipt sent={form.request} sentAt={form.sentAt} />
    : <EmailConfirmDraft form={form} />;
}

/** Read-only recipients as a compact grouped card so the editable Subject/Body
 *  stay the focus. Shared by the draft and the receipt — on the receipt the
 *  subject joins it, since nothing there is editable any more. */
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
  // An SMTP send is non-idempotent and can take tens of seconds (attachment
  // upload) — repeated Send clicks would deliver duplicate emails. The ref
  // carries the double-submit guard (a render-captured boolean would let two
  // rapid clicks in the same render window both read `false`); the state twin
  // drives the disabled/label rendering. Across a REMOUNT the guard is the
  // form's own `sentAt` instead: a sent email renders the receipt, which has no
  // Send button at all.
  const sendingRef = useRef(false);
  const [sending, setSending] = useState(false);

  // `void`: driveSend never rejects — every outcome is handled inside it.
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
        });
      },
      toast: showToast,
      // Keeps the user on this panel — the send confirmation is the panel
      // itself, showing what actually went out. No-ops (→ toast) if THIS draft
      // is no longer the active form: the buttons are disabled while sending,
      // but Escape still dismisses the panel, and a late success (the send
      // window is up to ~120s) must not resurrect it over an unrelated
      // form/overlay the user opened since.
      markSent: () => markEmailSent(form, { subject, body }),
    });
  };

  return (
    <div class="inline-form email-confirm">
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
        <button type="button" class="btn-cancel" onClick={() => closeInlineForm()} disabled={sending}>Cancel</button>
        <button type="button" class="btn-save" onClick={handleSend} disabled={sending}>{sending ? 'Sending…' : 'Send Email'}</button>
      </div>
    </div>
  );
}

/** The panel after a successful send: the same page, read-only, showing exactly
 *  what went out. It stays on screen instead of closing so the send has a
 *  visible result, and it owns the panel's nav-history slot (relabelled "Email
 *  Sent" by `getFormTitle`) so the user can walk back to it later. Deliberately
 *  offers no Send and no Cancel — the email is gone, the only action left is to
 *  close the panel and return to whatever view was underneath. */
function EmailSentReceipt({ sent, sentAt }: { sent: EmailConfirmForm['request']; sentAt: string }) {
  return (
    <div class="inline-form email-confirm">
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
