-- A credential's scope becomes a set of base URLs.
--
-- `base_url TEXT` held one value, and the proxy scope gate (ADR 0157) presents
-- a credential only where that value covers the request. Some providers split
-- one key across several hostnames, so the gate refused calls the user set up
-- on purpose: `api.binance.com` and `fapi.binance.com` share one HMAC pair, and
-- the futures entry answered 502 on every call. One `base_url` cannot express
-- it at all, so there was no setting to change and no recovery path.
--
-- `base_urls TEXT[]` replaces it. A request passes when ANY member covers it,
-- judged by the same `credential_base_url_matches` as before. There is no
-- wildcard and no suffix rule: each member is an exact scheme, host, effective
-- port and path prefix, and the user names every host.
--
-- The fill writes AT MOST ONE member per row, which is what keeps the upgrade
-- lossless in both directions. A row scoped to X comes out as {X}. A row whose
-- scope was blank comes out as {}, which the gate still refuses everywhere, the
-- same as the blank did. Nothing gains a host it did not already have.

ALTER TABLE credentials ADD COLUMN base_urls TEXT[] NOT NULL DEFAULT '{}';

UPDATE credentials
SET base_urls = CASE
        WHEN btrim(COALESCE(base_url, '')) = '' THEN ARRAY[]::TEXT[]
        ELSE ARRAY[btrim(base_url)]
    END;

ALTER TABLE credentials DROP COLUMN base_url;
