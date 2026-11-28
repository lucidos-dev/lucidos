import { useEffect, useState } from 'preact/hooks';
import { fetchEventHooks, fetchHookChoices, type EventHook, type HookChoice } from '../../api/client/eventHooks';
import { updateComposeSelection } from '../../store/actions/compose';
import { getComposeSelectionOverride } from '../../store/composeSelections';
import { eventHooksVersion, focusedThreadId } from '../../store/store';
import { loadingIfFresh, toFailed, type Loadable } from '../../store/types';
import { Dropdown } from '../shared/Dropdown';
import { LoadableError } from '../shared/LoadableError';

export function firstMessageArguments(hooks: EventHook[]) {
  return hooks.flatMap(hook => {
    if (hook.status !== 'valid' || hook.choices_timeout_secs === undefined) return [];
    const timeoutSecs = hook.choices_timeout_secs;
    return Object.entries(hook.definition.arguments ?? {}).map(([name, arg]) => ({
      slug: hook.slug,
      hash: hook.content_hash,
      name,
      label: arg.label,
      timeoutSecs,
    }));
  });
}

type DeclaredArgument = ReturnType<typeof firstMessageArguments>[number];

export function ComposeHookArguments({ threadId, fading }: { threadId: string | null; fading: boolean }) {
  const version = eventHooksVersion.value;
  const [args, setArgs] = useState<Loadable<DeclaredArgument[]>>({ status: 'not-loaded' });
  const [attempt, setAttempt] = useState(0);
  const values = getComposeSelectionOverride(threadId).hookArgs;
  useEffect(() => {
    let current = true;
    setArgs(loadingIfFresh);
    void fetchEventHooks().then(
      hooks => { if (current) setArgs({ status: 'loaded', data: firstMessageArguments(hooks) }); },
      error => { if (current) setArgs(toFailed(error)); },
    );
    return () => { current = false; };
  }, [version, attempt]);

  function pick(name: string, value: string) {
    if (focusedThreadId.peek() !== threadId) return;
    const next = { ...getComposeSelectionOverride(threadId).hookArgs };
    if (value) next[name] = value;
    else delete next[name];
    updateComposeSelection(threadId, { hookArgs: Object.keys(next).length ? next : undefined });
  }

  if (args.status === 'failed') {
    return <LoadableError error={args.error} noun="event hook arguments" onRetry={() => setAttempt(n => n + 1)} />;
  }
  if (args.status !== 'loaded') return null;

  return <>{args.data.map(arg => (
    <div key={JSON.stringify([arg.slug, arg.hash, arg.name, threadId])} class="compose-destination-row">
      <HookArgumentPicker
        argument={arg}
        value={values && Object.prototype.hasOwnProperty.call(values, arg.name) ? values[arg.name] : ''}
        disabled={fading}
        onChange={value => pick(arg.name, value)}
      />
    </div>
  ))}</>;
}

export function HookArgumentPicker({ argument, value, disabled, onChange, initialChoices = [] }: {
  argument: Omit<DeclaredArgument, 'hash'>;
  value: string;
  disabled: boolean;
  onChange: (value: string) => void;
  initialChoices?: HookChoice[];
}) {
  const [query, setQuery] = useState<string | null>(null);
  const [result, setResult] = useState<{ query: string; choices: Loadable<HookChoice[]> }>();
  const [selected, setSelected] = useState<HookChoice>();
  useEffect(() => {
    if (query === null || disabled) return;
    const controller = new AbortController();
    const timer = setTimeout(() => {
      void fetchHookChoices(argument.slug, argument.name, query, argument.timeoutSecs, controller.signal).then(
        choices => {
          if (!controller.signal.aborted) setResult({ query, choices: { status: 'loaded', data: choices } });
        },
        (error: unknown) => {
          if (!controller.signal.aborted) setResult({ query, choices: toFailed(error) });
        },
      );
    }, query ? 200 : 0);
    return () => { clearTimeout(timer); controller.abort(); };
  }, [query, disabled, argument.slug, argument.name, argument.timeoutSecs]);

  function search(nextQuery: string) {
    if (query === nextQuery) return;
    setResult(undefined);
    setQuery(nextQuery);
  }

  const current = result && result.query === query ? result.choices : undefined;
  const choices = current?.status === 'loaded' ? current.data : query === '' ? initialChoices : [];
  const label = choices.find(choice => choice.value === value)?.label
    ?? (selected?.value === value ? selected.label : value);
  const options = [...choices];
  if (value && query === '') options.unshift({ value: '', label: 'Clear selection' });
  const enteredValue = query?.trim();
  if (current?.status === 'failed' && enteredValue) {
    options.push({ value: enteredValue, label: `Use “${enteredValue}”` });
  }
  const error = current?.status === 'failed'
    ? `${current.error} Type a known value to use it.`
    : undefined;

  return <Dropdown
    class="compose-destination-picker"
    ariaLabel={value ? `${argument.label}: ${label}` : argument.label}
    placeholder={label || argument.label}
    value={value}
    options={options}
    disabled={disabled}
    onChange={next => {
      setSelected(choices.find(choice => choice.value === next));
      onChange(next);
    }}
    onClose={() => setQuery(null)}
    onSearch={search}
    searchStatus={current ? error : null}
  />;
}
