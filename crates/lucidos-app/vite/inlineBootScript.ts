/**
 * Put the appearance boot bundle into `index.html` in place of `marker`, as an
 * inline `<script>`. Used by the `lucidos-appearance-boot` Vite plugin.
 *
 * The bundle goes in through a replacer FUNCTION, never a replacement string.
 * A string reads `$&`, `` $` `` and `$'` as patterns. A template literal ending
 * in a regex anchor spells `` $` ``, and that pasted the page head into the
 * script, which then rendered as text.
 */
export function inlineBootScript(html: string, marker: string, bundle: string): string {
  // An HTML parser ends a <script> at the first `</script`, even inside a
  // string or a regex literal. The bundle would then close its own tag and
  // spill the rest into the document as markup.
  if (/<\/script/i.test(bundle)) {
    throw new Error(
      'The appearance boot bundle contains `</script`, which would terminate the '
      + 'inline tag early. Rewrite the source so the sequence cannot appear '
      + '(e.g. split the string).',
    );
  }
  return html.replace(marker, () => `<script>\n${bundle}</script>`);
}
