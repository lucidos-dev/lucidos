export function inlineBootScript(html: string, marker: string, bundle: string): string {
  if (/<\/script/i.test(bundle)) {
    throw new Error(
      'The appearance boot bundle contains `</script`, which would terminate the '
      + 'inline tag early. Rewrite the source so the sequence cannot appear '
      + '(e.g. split the string).',
    );
  }
  return html.replace(marker, () => `<script>\n${bundle}</script>`);
}
