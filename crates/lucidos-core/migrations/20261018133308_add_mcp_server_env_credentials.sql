-- An MCP server env entry can name a credential instead of holding a value.
-- The pointer is a credential name, never a secret, so it lives in its own
-- plain column beside the sealed `env`: variable name to credential name.
--
-- NULL marks a row written before pointers existed. Its env values have not
-- moved into credentials yet, which the engine does in Rust at its next boot
-- (`secret_store::one_home`), since a migration sees only ciphertext.
-- Every write after this sets the column, so a value added later stays put.
ALTER TABLE mcp_servers ADD COLUMN env_credentials JSONB;
