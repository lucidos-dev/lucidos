-- The Claude gateway tokens are sealed at rest too (ADR 0509). Both columns
-- join core::secret_store::SECRET_COLUMNS with a CHECK named
-- <table>_<column>_sealed.
--
-- NOT VALID for the same reason as 20261017194334_seal_stored_secrets.sql: a
-- row already here stays plain until the boot seals it and validates.

ALTER TABLE claude_gateway_accounts ADD CONSTRAINT claude_gateway_accounts_access_token_sealed
    CHECK (access_token LIKE 'lsec1:%') NOT VALID;
ALTER TABLE claude_gateway_accounts ADD CONSTRAINT claude_gateway_accounts_refresh_token_sealed
    CHECK (refresh_token LIKE 'lsec1:%') NOT VALID;
