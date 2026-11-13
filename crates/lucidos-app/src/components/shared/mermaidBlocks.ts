import { mermaidFrameDocument } from '../../utils/mermaidFrame';
import { loadMermaidLibrary } from '../../utils/mermaidLibrary';
import { generateUuid } from '../../utils/uuid';
import { resolvedTokenHex } from '../../utils/cssColor';
import '../../styles/chat/mermaid.css';

/** Enhance only genuine renderer-owned code blocks in completed replies.
 * Keep the original pre/code and Copy button, including its delegated handler. */
export function enhanceMermaidBlocks(host: HTMLElement): () => void {
    const cleanups: (() => void)[] = [];
    for (const block of host.querySelectorAll<HTMLElement>('.code-block-wrapper[data-copy-code]')) {
      const header = block.querySelector<HTMLElement>(':scope > .code-block-header');
      const label = header?.querySelector<HTMLElement>('.code-block-lang');
      const pre = block.querySelector<HTMLPreElement>(':scope > pre');
      if (label?.textContent?.toLowerCase() !== 'mermaid' || !header || !pre) continue;
      const source = pre.querySelector('code')?.textContent ?? '';
      const controls = document.createElement('div');
      controls.className = 'segmented-control';
      controls.setAttribute('role', 'group');
      controls.setAttribute('aria-label', 'Mermaid view');
      const frame = document.createElement('iframe');
      frame.title = 'Mermaid diagram';
      frame.setAttribute('sandbox', 'allow-scripts');
      frame.setAttribute('referrerpolicy', 'no-referrer');
      frame.className = 'mermaid-frame';
      const notice = document.createElement('p');
      notice.className = 'mermaid-notice';
      notice.setAttribute('role', 'status');
      notice.textContent = 'Rendering diagram…';
      let codeView = false;
      let failed = false;
      let disposed = false;
      let ready = false;
      const channel = generateUuid();
      const buttons = ['Diagram', 'Code'].map((name, index) => {
        const button = document.createElement('button');
        button.type = 'button';
        button.className = 'segmented-btn';
        button.textContent = name;
        button.onclick = () => { codeView = index === 1; update(); send(); };
        controls.append(button);
        return button;
      });
      function update() {
        buttons.forEach((button, i) => {
          const selected = codeView === (i === 1);
          button.classList.toggle('active', selected);
          button.setAttribute('aria-pressed', String(selected));
        });
        pre!.hidden = !codeView && !failed;
        frame.hidden = codeView || failed;
      }
      function fail() {
        failed = true;
        codeView = true;
        buttons[0].disabled = true;
        notice.hidden = false;
        notice.textContent = 'Could not render this Mermaid diagram. The code is available below.';
        update();
      }
      let lastPayload = '';
      function send() {
        if (!ready || disposed || failed || codeView) return;
        const style = getComputedStyle(document.documentElement);
        const color = (token: `--${string}`) => resolvedTokenHex(token) ?? style.getPropertyValue(token).trim();
        const payload = { channel, source, motion: document.documentElement.getAttribute('data-motion'), theme: {
          background: color('--bg-primary'), primaryColor: color('--bg-tertiary'),
          primaryTextColor: color('--text-primary'), textColor: color('--text-primary'),
          primaryBorderColor: color('--border-color'), lineColor: color('--text-muted'),
          secondaryColor: color('--bg-secondary'), tertiaryColor: color('--bg-secondary'),
          fontFamily: style.getPropertyValue('--font-ui').trim(),
          fontSize: style.fontSize,
        } };
        const serialized = JSON.stringify(payload);
        if (serialized === lastPayload) return;
        lastPayload = serialized;
        frame.contentWindow?.postMessage(payload, '*');
      }
      const receive = (event: MessageEvent) => {
        if (disposed || failed || event.source !== frame.contentWindow || event.data?.channel !== channel) return;
        const { status, height } = event.data;
        if (status === 'ready') { ready = true; send(); }
        else if (status === 'error') { clearTimeout(timeout); fail(); }
        else if (status === 'rendered') {
          clearTimeout(timeout);
          notice.hidden = true;
        }
        if (typeof height === 'number' && Number.isFinite(height)) {
          // Browser geometry, not a design size; cap huge graphs to a scrollable frame.
          frame.style.height = `${Math.max(100, Math.min(2000, height))}px`;
        }
      };
      addEventListener('message', receive);
      const timeout = setTimeout(fail, 30000);
      const observer = new MutationObserver(send);
      observer.observe(document.documentElement, { attributes: true, attributeFilter: ['style', 'data-theme', 'data-theme-mode', 'data-motion'] });
      label.replaceWith(controls);
      block.classList.add('mermaid-block');
      block.append(notice, frame);
      update();
      void loadMermaidLibrary().then((library) => {
        if (!disposed && !failed) frame.srcdoc = mermaidFrameDocument(library, channel);
      }).catch(() => { if (!disposed) fail(); });
      cleanups.push(() => {
        disposed = true;
        clearTimeout(timeout);
        observer.disconnect();
        removeEventListener('message', receive);
        controls.replaceWith(label);
        frame.remove();
        notice.remove();
        pre.hidden = false;
        block.classList.remove('mermaid-block');
      });
    }
    return () => { for (const cleanup of cleanups) cleanup(); };
}
