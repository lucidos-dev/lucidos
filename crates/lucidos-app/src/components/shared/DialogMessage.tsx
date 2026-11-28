export function dialogParagraphs(message: string): string[] {
  const parts = message.split(/\n[ \t\r]*\n/).map((p) => p.trim()).filter(Boolean);
  return parts.length > 0 ? parts : [''];
}

export function DialogMessage({ message }: { message: string }) {
  return (
    <>
      {dialogParagraphs(message).map((para, i) => (
        <p class="confirm-message" key={i}>{para}</p>
      ))}
    </>
  );
}
