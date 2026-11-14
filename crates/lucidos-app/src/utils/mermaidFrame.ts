import template from './mermaidFrame.html?raw';

/** The static frame runs in an opaque origin; theme data travels by message. */
export function mermaidFrameDocument(library: string, nonce: string): string {
  const script = library.replace(/<\/script/gi, '<\\/script');
  return template.replace(/__MERMAID_NONCE__/g, () => nonce).replace('__MERMAID_LIBRARY__', () => script);
}
