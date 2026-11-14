import { useLayoutEffect } from 'preact/hooks';

/** Load diagram machinery only for completed replies containing Mermaid. */
export function useMermaidBlocks(host: HTMLElement | null, html: string, final: boolean): void {
  useLayoutEffect(() => {
    if (!host || !final || !host.querySelector('.code-block-lang') || !html.toLowerCase().includes('mermaid')) return;
    let disposed = false;
    let cleanup: (() => void) | undefined;
    void import('./mermaidBlocks').then(({ enhanceMermaidBlocks }) => {
      if (!disposed) cleanup = enhanceMermaidBlocks(host);
    }).catch(() => {
      if (disposed) return;
      const notice = document.createElement('p');
      notice.setAttribute('role', 'status');
      notice.textContent = 'Could not load Mermaid diagrams. The code is available above.';
      host.append(notice);
      cleanup = () => notice.remove();
    });
    return () => { disposed = true; cleanup?.(); };
  }, [host, html, final]);
}
