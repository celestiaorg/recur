-- Context window size reported (or resolved) for the turn that stamped this
-- message. Paired with input/cache token columns so the chat can show a
-- used/limit context meter after reload.
ALTER TABLE message ADD COLUMN context_window INTEGER;
