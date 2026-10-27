-- Existing keys retain their original permissions. Setup is an explicit opt-in.
ALTER TABLE workspace_api_keys DROP CONSTRAINT workspace_api_keys_scopes_check;
ALTER TABLE workspace_api_keys ADD CONSTRAINT workspace_api_keys_scopes_check
CHECK(scopes <@ ARRAY['read','leads:write','bookings:write','numbers:write','setup:write']::text[] AND 'read'=ANY(scopes));
