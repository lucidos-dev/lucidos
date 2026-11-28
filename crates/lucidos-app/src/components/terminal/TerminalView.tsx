import { useEffect, useRef, useState } from 'preact/hooks';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import './terminal.css';
import {
  getTerminalStatus, isTerminalDisabled, killTerminal, mintTerminalTicket, openTerminal,
  type TerminalSize,
} from '../../api/client/terminal';
import { errorDetail } from '../../utils/errorDetail';
import { showConfirm, showToast, threadMap } from '../../store/store';
import { appearanceVersion, currentTerminalEnabled } from '../../store/actions/preferences';
import { openSettingsSubview } from '../../store/actions/menu';
import { takeTerminalOpenIntent, threadCanHaveTerminal } from '../../store/actions/terminal';
import { viewportIsMobile } from '../../utils/viewport';
import { onPageResume } from '../../utils/pageResume';
import { copyToClipboard, clipboardOrReport } from '../../utils/clipboard';
import { terminalSocketUrl } from '../../utils/socketUrl';
import { blockedRelayGateway, RelayBlockedError } from '../../utils/socketProbe';
import { RelayBlockedExplainer } from './RelayBlockedExplainer';
import { TerminalConnection, type ConnectionState } from '../../terminal/connection';
import { terminalTheme } from '../../terminal/theme';
import { installHyperlinks, labelShowsTarget } from '../../terminal/hyperlinks';
import { openUrlOutsideApp } from '../../store/actions/artifacts';
import { CTRL_KEY_ID, KEY_ROW, PASTE_KEY_ID, withControl, type KeyRowKey } from '../../terminal/keyRow';
import { exitCauseText, terminalStatusText, type TerminalPhase } from '../../terminal/phase';
import { CopyIcon } from '../shared/icons';

const RESIZE_DEBOUNCE_MS = 100;

interface Live {
  terminalId: string;
  connection: TerminalConnection;
}

function visibleText(term: Terminal): string {
  if (term.hasSelection()) return term.getSelection();
  const buffer = term.buffer.active;
  const lines: string[] = [];
  for (let row = 0; row < term.rows; row++) {
    lines.push(buffer.getLine(buffer.viewportY + row)?.translateToString(true) ?? '');
  }
  return lines.join('\n').replace(/\s+$/, '');
}

function fontSizePx(host: HTMLElement): number {
  return parseFloat(getComputedStyle(host).fontSize) || 13;
}

const LINK_SOURCE = 'the terminal';

async function openTerminalLink(uri: string, label: string): Promise<void> {
  if (!labelShowsTarget(label, uri)) {
    const message = `This terminal link shows "${label.trim()}" but goes to:\n\n${uri}`;
    if (!(await showConfirm(message, 'Open link', { variant: 'default' }))) return;
  }
  openUrlOutsideApp(uri, LINK_SOURCE);
}

function failurePhase(err: unknown): TerminalPhase {
  if (isTerminalDisabled(err)) return { kind: 'off' };
  if (err instanceof RelayBlockedError) return { kind: 'relay-blocked', gateway: err.gateway };
  return { kind: 'failed', message: errorDetail(err) };
}

export function TerminalView({ threadId, layout }: { threadId: string; layout: 'desktop' | 'mobile' }) {
  const activeLayout = layout === (viewportIsMobile.value ? 'mobile' : 'desktop');
  if (!activeLayout) return null;
  return <TerminalSurface key={threadId} threadId={threadId} mobile={layout === 'mobile'} />;
}

function TerminalSurface({ threadId, mobile }: { threadId: string; mobile: boolean }) {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const liveRef = useRef<Live | null>(null);
  const ctrlRef = useRef(false);
  const [phase, setPhase] = useState<TerminalPhase>({ kind: 'checking' });
  const [attachments, setAttachments] = useState(0);
  const [ctrlArmed, setCtrlArmed] = useState(false);

  const meta = threadMap.value.get(threadId)?.meta;
  const switchOn = currentTerminalEnabled();
  const canRespawn = switchOn && !!meta && threadCanHaveTerminal(meta);

  function armCtrl(armed: boolean): void {
    ctrlRef.current = armed;
    setCtrlArmed(armed);
  }

  function ownSize(): TerminalSize {
    const term = termRef.current;
    fitRef.current?.fit();
    return { cols: term?.cols ?? 80, rows: term?.rows ?? 24 };
  }

  function detach(): void {
    liveRef.current?.connection.dispose();
    liveRef.current = null;
  }

  function attach(terminalId: string, ticket: string | null): void {
    detach();
    const term = termRef.current;
    if (!term) return;
    const connection = new TerminalConnection(
      {
        mintTicket: async () => (await mintTerminalTicket(threadId, terminalId)).ticket,
        socketUrl: (t) => terminalSocketUrl(threadId, terminalId, t),
        openSocket: (url) => new WebSocket(url),
        schedule: (run, ms) => {
          const timer = setTimeout(run, ms);
          return () => clearTimeout(timer);
        },
      },
      {
        write: (data) => term.write(data),
        reset: () => term.reset(),
        size: ({ cols, rows }) => { if (cols !== term.cols || rows !== term.rows) term.resize(cols, rows); },
        attachments: setAttachments,
        state: (state: ConnectionState) => setPhase(state.kind === 'disabled' ? { kind: 'off' } : { kind: 'live', state }),
      },
      ownSize(),
      ticket,
    );
    liveRef.current = { terminalId, connection };
    connection.start();
  }

  async function startShell(): Promise<void> {
    setPhase({ kind: 'checking' });
    try {
      const opened = await openTerminal(threadId, ownSize());
      termRef.current?.reset();
      attach(opened.terminal_id, opened.ticket);
      termRef.current?.focus();
    } catch (err) {
      setPhase(failurePhase(err));
    }
  }

  async function check(): Promise<void> {
    try {
      const status = await getTerminalStatus(threadId);
      if (!status.enabled) {
        setPhase({ kind: 'off' });
      } else if (status.terminal) {
        const gateway = await blockedRelayGateway();
        if (gateway) setPhase({ kind: 'relay-blocked', gateway });
        else attach(status.terminal.terminal_id, null);
      } else if (takeTerminalOpenIntent(threadId)) {
        await startShell();
      } else {
        setPhase({ kind: 'none' });
      }
    } catch (err) {
      setPhase({ kind: 'failed', message: errorDetail(err) });
    }
  }

  async function kill(): Promise<void> {
    const live = liveRef.current;
    if (!live) return;
    try {
      await killTerminal(threadId, live.terminalId);
    } catch (err) {
      showToast(`Could not end the shell: ${errorDetail(err)}`, 'error');
    }
  }

  async function paste(): Promise<void> {
    const clipboard = clipboardOrReport();
    if (!clipboard) return;
    try {
      termRef.current?.paste(await clipboard.readText());
    } catch (err) {
      showToast(`Could not read the clipboard: ${errorDetail(err)}`, 'error');
    }
  }

  function pressKey(key: KeyRowKey): void {
    if (key.id === CTRL_KEY_ID) {
      armCtrl(!ctrlRef.current);
      return;
    }
    if (key.id === PASTE_KEY_ID) {
      void paste();
      return;
    }
    if (key.bytes) liveRef.current?.connection.input(key.bytes);
    armCtrl(false);
  }

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const style = getComputedStyle(host);
    const term = new Terminal({
      cursorBlink: true,
      fontFamily: style.getPropertyValue('--font-mono').trim() || 'monospace',
      fontSize: fontSizePx(host),
      scrollback: 5000,
      theme: terminalTheme(),
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    const hyperlinks = installHyperlinks(term, openTerminalLink);
    term.open(host);
    const textarea = term.textarea;
    if (textarea) {
      textarea.setAttribute('autocapitalize', 'off');
      textarea.setAttribute('autocorrect', 'off');
      textarea.setAttribute('autocomplete', 'off');
      textarea.spellcheck = false;
    }
    termRef.current = term;
    fitRef.current = fit;
    fit.fit();

    const send = (data: string) => {
      const typed = ctrlRef.current ? withControl(data) : data;
      if (ctrlRef.current) armCtrl(false);
      liveRef.current?.connection.input(typed);
    };
    const onData = term.onData(send);
    const onBinary = term.onBinary((data) => {
      liveRef.current?.connection.input(Uint8Array.from(data, (c) => c.charCodeAt(0)));
    });
    const onFocus = () => {
      fit.fit();
      const connection = liveRef.current?.connection;
      connection?.resize({ cols: term.cols, rows: term.rows });
      connection?.focus();
    };
    textarea?.addEventListener('focus', onFocus);

    let resizeTimer: ReturnType<typeof setTimeout> | undefined;
    const observer = new ResizeObserver(() => {
      clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => {
        fit.fit();
        liveRef.current?.connection.resize({ cols: term.cols, rows: term.rows });
      }, RESIZE_DEBOUNCE_MS);
    });
    observer.observe(host);
    const stopResume = onPageResume(() => liveRef.current?.connection.resumeNow());

    void check();

    return () => {
      clearTimeout(resizeTimer);
      observer.disconnect();
      stopResume();
      textarea?.removeEventListener('focus', onFocus);
      onData.dispose();
      onBinary.dispose();
      hyperlinks.dispose();
      detach();
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
  }, []);

  useEffect(() => {
    const term = termRef.current;
    if (term) term.options.theme = terminalTheme();
  }, [appearanceVersion.value]);

  const live = phase.kind === 'live' && (phase.state.kind === 'attached' || phase.state.kind === 'reconnecting');
  const shown: TerminalPhase = switchOn ? phase : { kind: 'off' };
  const where = meta?.channel === 'claude_code' ? 'Worktree' : 'Workspace data';
  const status = terminalStatusText(shown, { where, attachments, mobile });
  const offerNewShell = canRespawn && (shown.kind === 'none' || shown.kind === 'failed'
    || (shown.kind === 'live' && (shown.state.kind === 'ended' || shown.state.kind === 'gone')));

  return (
    <div class="content-view-full-bleed active terminal-view" data-phase={shown.kind === 'live' ? shown.state.kind : shown.kind}>
      <div class="terminal-status-line">
        <span class="terminal-status-text" data-role="terminal-status">
          {live && <span class="terminal-live-dot" aria-hidden="true" />}
          {status}
          {shown.kind === 'live' && shown.state.kind === 'ended' && (
            <span class="terminal-exit-cause"> {exitCauseText(shown.state.cause, shown.state.status)}</span>
          )}
        </span>
        {shown.kind === 'relay-blocked' && <RelayBlockedExplainer gateway={shown.gateway} />}
        {shown.kind === 'off' && (
          <button type="button" class="accent-link" onClick={() => openSettingsSubview('developer')}>
            Open Settings
          </button>
        )}
        {live && (
          <>
            <button type="button" class="icon-btn" aria-label="Copy" data-tooltip="Copy the selection or the screen"
              onClick={() => { const term = termRef.current; if (term) copyToClipboard(visibleText(term)); }}>
              <CopyIcon />
            </button>
            <button type="button" class="action-btn action-btn-danger" onClick={() => void kill()}>Kill</button>
          </>
        )}
        {shown.kind === 'relay-blocked' && (
          <button type="button" class="action-btn" onClick={() => { setPhase({ kind: 'checking' }); void check(); }}>
            Try again
          </button>
        )}
        {offerNewShell && (
          <button type="button" class="action-btn" onClick={() => void startShell()}>New shell</button>
        )}
      </div>
      <div class="terminal-screen" ref={hostRef} />
      {mobile && live && (
        <div class="terminal-key-row" role="toolbar" aria-label="Terminal keys">
          {KEY_ROW.map((key) => (
            <button key={key.id} type="button" class="terminal-key" aria-label={key.ariaLabel}
              aria-pressed={key.id === CTRL_KEY_ID ? ctrlArmed : undefined}
              onPointerDown={(e) => { e.preventDefault(); pressKey(key); }}
              onClick={(e) => { if (e.detail === 0) pressKey(key); }}>
              {key.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
