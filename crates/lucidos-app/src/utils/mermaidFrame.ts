import template from './mermaidFrame.html?raw';

export function mermaidFrameDocument(library: string, nonce: string): string {
  const script = library.replace(/<\/script/gi, '<\\/script');
  return template.replace(/__MERMAID_NONCE__/g, () => nonce).replace('__MERMAID_LIBRARY__', () => script);
}
