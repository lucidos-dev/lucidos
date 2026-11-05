-- Link email accounts to OAuth accounts for XOAUTH2 SMTP authentication
ALTER TABLE email_accounts ADD COLUMN oauth_account_id UUID REFERENCES oauth_accounts(id) ON DELETE SET NULL;
