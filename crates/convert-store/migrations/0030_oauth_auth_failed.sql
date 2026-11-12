-- When the provider last rejected this connection's refresh token (revoked, expired, or
-- the user removed access). Set by the token refresh for the revision that failed, and
-- cleared by the next successful refresh, a reconnect or a disconnect. While it is set the
-- connection reports reconnect_required instead of connected. Additive and nullable.
ALTER TABLE google_connections ADD COLUMN auth_failed_at timestamptz;
ALTER TABLE microsoft_connections ADD COLUMN auth_failed_at timestamptz;
