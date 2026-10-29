-- Recur does not track USD. Subscription usage is not API list price.
ALTER TABLE message DROP COLUMN cost_usd;
