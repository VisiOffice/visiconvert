-- Activity log (GET /api/v1/activity): newest-first reads per workspace. Additive only:
-- new indexes, no table or column changes. Leads and pipeline events already have one.
CREATE INDEX conversion_deliveries_recent ON conversion_deliveries(workspace_id, created_at DESC, id DESC);
CREATE INDEX microsoft_deliveries_recent ON microsoft_deliveries(workspace_id, created_at DESC, id DESC);
CREATE INDEX webhook_deliveries_recent ON webhook_deliveries(workspace_id, created_at DESC, id DESC);
CREATE INDEX webhook_endpoint_events_recent ON webhook_endpoint_events(workspace_id, created_at DESC, id DESC);
-- Only the audit rows the log shows; worker and trigger rows restate other sources.
CREATE INDEX conversion_events_activity ON conversion_events(workspace_id, created_at DESC, id DESC)
WHERE actor NOT IN ('worker','system')
  AND kind NOT IN ('delivery_status','invalidated','pipeline_event_recorded','pipeline_event_canceled');
