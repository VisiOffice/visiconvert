-- conversions:send lets an owner-created key or MCP grant switch Google delivery and
-- automatic sending and attest the consent policy on the owner's behalf. Additive only.
ALTER TABLE workspace_api_keys DROP CONSTRAINT workspace_api_keys_scopes_check;
ALTER TABLE workspace_api_keys ADD CONSTRAINT workspace_api_keys_scopes_check
CHECK(scopes <@ ARRAY['read','leads:write','bookings:write','numbers:write','numbers:purchase','setup:write','webhooks:write','conversions:write','conversions:send']::text[] AND 'read'=ANY(scopes));
