-- 0020_message_token_totals: tokens billed for the whole turn, on the message
-- that ends it. input_tokens and friends hold the context meter's last-request
-- footprint, which under-reports a multi-request turn. Rows written before
-- this migration stay NULL: their totals were never recorded.

ALTER TABLE message ADD COLUMN total_input_tokens INTEGER;
ALTER TABLE message ADD COLUMN total_output_tokens INTEGER;
ALTER TABLE message ADD COLUMN total_cache_creation_tokens INTEGER;
ALTER TABLE message ADD COLUMN total_cache_read_tokens INTEGER;
