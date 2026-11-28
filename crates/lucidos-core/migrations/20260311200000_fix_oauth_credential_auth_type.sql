-- Fix legacy oauth:* credentials that were stored with wrong auth_type
UPDATE credentials SET auth_type = 'oauth_client' WHERE service_name LIKE 'oauth:%' AND auth_type != 'oauth_client';
