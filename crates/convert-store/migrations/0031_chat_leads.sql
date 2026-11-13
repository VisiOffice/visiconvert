-- Web chat leads posted by a chat system (POST /api/v1/chat-leads), linked to the
-- website visit that carried their ad click like a form submission.
ALTER TABLE leads DROP CONSTRAINT leads_channel_check;
ALTER TABLE leads ADD CONSTRAINT leads_channel_check CHECK (channel IN ('call', 'form', 'chat'));

-- Normalized identifiers for pipeline matching (match.phone / match.email), kept apart
-- from the display `contact` so a lead can be matched by both its phone and its email.
-- Phone is the last 10 digits; email is trimmed and lowercased. Older leads keep matching
-- on `contact` as before.
ALTER TABLE leads
    ADD COLUMN match_phone text CHECK (match_phone ~ '^[0-9]{10}$'),
    ADD COLUMN match_email text CHECK (match_email = lower(btrim(match_email)) AND length(match_email) <= 254);
CREATE INDEX leads_match_phone ON leads(workspace_id, match_phone) WHERE match_phone IS NOT NULL;
CREATE INDEX leads_match_email ON leads(workspace_id, match_email) WHERE match_email IS NOT NULL;
