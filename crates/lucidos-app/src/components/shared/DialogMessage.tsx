/** Splits a dialog message into paragraphs on BLANK lines.
 *
 *  A single newline is not a break: dialog copy is assembled from concatenated
 *  source strings, where the line ends are an artifact of the source file's
 *  width and collapse to a space the way HTML would collapse them. Only a
 *  deliberate blank line (`\n\n`) starts a new paragraph.
 *
 *  `\r` counts as separator whitespace, so a CRLF-authored message (an app
 *  passing `\r\n\r\n` through `lucidos.ui.confirm`) breaks the same way an
 *  LF one does.
 *
 *  Pure and exported so the split is unit-testable without rendering. */
export function dialogParagraphs(message: string): string[] {
  const parts = message.split(/\n[ \t\r]*\n/).map((p) => p.trim()).filter(Boolean);
  // An empty (or whitespace-only) message still renders one empty paragraph, so
  // the dialog keeps its message slot and its spacing rather than collapsing.
  return parts.length > 0 ? parts : [''];
}

/** The message body shared by `ConfirmDialog` and `PromptDialog` (which
 *  deliberately share the same chrome). One `.confirm-message` paragraph per
 *  blank-line-separated block. */
export function DialogMessage({ message }: { message: string }) {
  return (
    <>
      {dialogParagraphs(message).map((para, i) => (
        <p class="confirm-message" key={i}>{para}</p>
      ))}
    </>
  );
}
