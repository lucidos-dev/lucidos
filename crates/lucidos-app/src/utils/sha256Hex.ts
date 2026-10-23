/** The lowercase hex SHA-256 of `bytes`, as the engine's blob store names them
 *  (`core/blobs.rs` `compute_hash`). Undefined where Web Crypto is missing,
 *  which is any insecure context: plain `http` off localhost. */
export async function sha256Hex(bytes: BufferSource): Promise<string | undefined> {
  const subtle = globalThis.crypto?.subtle;
  if (!subtle) return undefined;
  const digest = await subtle.digest('SHA-256', bytes);
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, '0')).join('');
}
