-- Store the concrete service-worker scope for each browser push subscription.
-- iOS declarative push navigation is sensitive to the exact workspace URL; the
-- engine cannot infer that from APNs/FCM endpoints, so the page records it when
-- subscribing.
ALTER TABLE push_subscriptions ADD COLUMN IF NOT EXISTS scope_url TEXT;
