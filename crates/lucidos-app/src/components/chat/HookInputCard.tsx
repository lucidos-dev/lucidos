import { useState } from 'preact/hooks';
import type { HookInputOffer } from '../../api/client/eventHooks';
import { cancelUnsentHookInput, discardUnsentMessage, retryUnsentMessage } from '../../store/actions/chat';
import { errorDetail } from '../../utils/errorDetail';
import { HookArgumentPicker } from './ComposeHookArguments';

export function HookInputCard({ eventId, input, overtaken }: {
  eventId: string;
  input?: HookInputOffer;
  overtaken?: string;
}) {
  const [title, setTitle] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const creation = input?.creation;
  const newValue = title.trim();

  async function act(task: () => Promise<unknown>) {
    if (busy) return;
    setBusy(true);
    setError('');
    try {
      await task();
    } catch (error) {
      setError(errorDetail(error));
    } finally {
      setBusy(false);
    }
  }

  return <>
    {input && <HookArgumentPicker
      argument={{ slug: input.hook, name: input.argument, label: input.label, timeoutSecs: input.choices_timeout_secs }}
      initialChoices={input.choices}
      value=""
      disabled={busy}
      onChange={value => void act(async () => { await retryUnsentMessage(eventId, { kind: 'pick', value }); })}
    />}
    {creation && <input
      class="compose-destination-picker"
      aria-label={creation.label}
      placeholder={creation.label}
      value={title}
      maxLength={creation.max_bytes}
      disabled={busy}
      onInput={event => setTitle(event.currentTarget.value)}
    />}
    {overtaken && <p>{overtaken}</p>}
    {error && <p role="alert">{error}</p>}
    <div class="exchange-error-actions">
      <button type="button" class="action-btn action-btn-secondary" disabled={busy}
        onClick={() => void act(async () => { await cancelUnsentHookInput(eventId); })}>{overtaken ? 'Return to composer' : 'Cancel'}</button>
      {error && <button type="button" class="action-btn action-btn-secondary" disabled={busy}
        onClick={() => discardUnsentMessage(eventId)}>Discard locally</button>}
      {creation && <button type="button" class="action-btn"
        disabled={busy || !newValue || new TextEncoder().encode(newValue).length > creation.max_bytes}
        onClick={() => void act(async () => { await retryUnsentMessage(eventId, { kind: 'create', value: newValue }); })}>Create and send</button>}
    </div>
  </>;
}
