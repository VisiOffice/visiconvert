---
title: Google Ads conversion delivery
description: Review, validate and explicitly queue conversions for Google Ads.
---


This shared Rust/Vite feature is available in both hosted and self-hosted builds.
It prepares qualified inquiries and bookings for review and can deliver explicitly
queued events through Google's Data Manager API. **Delivery is off by default.**
An upgrade never queues existing preparations. See [Google connection](/guides/google-ads-connection/)
for OAuth and destination configuration. Microsoft delivery is not implemented.

## Workflow

1. Open **Conversions**. Save your 10-digit Google Ads customer ID without dashes.
   Map qualification and/or bookings to distinct existing numeric conversion action
   IDs. Use the Google connection controls to check account access and action compatibility.
   Saving IDs alone does not validate them. Choose your
   primary optimization action in Google Ads to avoid counting both funnel stages
   as separate sales.
2. Select a candidate. Qualified inquiries use their primary interaction; bookings
   retain the original interaction, even after combining or separating inquiries.
   The latest 200 candidates and preparations are shown, plus 100 history entries.
3. Review the captured click identifier, original recorded event time, and source.
   Record advertising consent only when you have evidence of an existing decision,
   with its original effective time and receipt/policy reference. This screen does
   not obtain consent. General website tracking consent never implies advertising
   consent. Keep personal information out of references.
4. Stage a reviewed eligible event. The frozen snapshot stays in PostgreSQL with a
   stable workspace/event transaction ID. Repeated staging requests cannot create
   duplicate jobs. Booking values remain integer minor units (USD cents).
5. Review again after changes. Booking changes, inquiry qualification/grouping,
   destination settings, and consent updates conservatively invalidate **all**
   awaiting preparations in that workspace. Frozen snapshots are preserved until
   explicitly restaged; cancellations and invalidations are auditable. Notes-only
   changes and identical booking retries do not invalidate preparations.

6. Validate the saved Google destination, then choose **Validate event with Google**
   on a preparation. This sends identifiers to Google's validation endpoint with
   `validateOnly=true`; it does not record a conversion. Destination checks expire
   after 24 hours; event validation expires after one hour before queuing.
7. Disable any overlapping CRM uploader for the same action. **Enable reviewed
   delivery**, then **Queue conversion for Google** for each validated event.
   There is no automatic enqueueing on qualification or booking in this slice.
8. Refresh delivery history to see submitted, processed, failed or uncertain status.
   Processed means request processing succeeded, not that Google matched the click,
   attributed a conversion, or used it in bidding reports. Check Google diagnostics.

## Delivery and revisions

The Rust server runs the worker in hosted and standalone modes whenever Google
OAuth is configured. It uses PostgreSQL leases and row locks; multiple instances
cannot claim the same active job. The worker rechecks current eligibility, consent,
connection revision and destination before every upload under the workspace write
lock. It sends one event per request and stores Google's request ID separately
from the stable conversion transaction ID. No contact details or consent references
are sent. USD values are constructed from integer cents.

429 responses and connection failures retry with bounded exponential backoff (five
attempts). Upload timeouts, server errors, malformed receipts and interrupted sends
become **uncertain**, since Google may have received them. They never automatically
replay. Status polling is read-only and retries up to 24 times, with a one-hour
maximum delay. **Recheck Google receipt** resumes polling an existing request ID
without sending the event again. A send with no receipt requires manual provider
investigation; this release has no unsafe reset-and-resend control.

After successful processing, a reviewed booking revision can update its value
using the original transaction ID, destination, identifiers and event timestamp.
A revision cannot overtake an in-flight, failed or uncertain earlier attempt.
Changing destination or identity after attempting delivery is blocked; this avoids
counting an existing event in another action. Qualification events cannot be resent.

**Google Data Manager does not support retractions.** Canceling a booking or
withdrawing consent stops unsent work and flags already-attempted delivery for
review. Existing receipts remain pollable. VisiConvert does not send a zero-value
conversion and call it a retraction. Correct already-sent conversions in Google;
automated retractions are not part of this release.

Pause cancels pending send eligibility and requires review before resuming.
Disconnecting Google prevents new sends; receipt polling may need reconnection.
Warnings, stopped jobs and source changes are visible in delivery history. Provider
error bodies are never exposed in the UI or logs.

## Eligibility and limitations

- A captured website visit and gclid, gbraid, or wbraid are required. No enhanced
  conversion contact matching or cross-device inference is implemented.
- Advertising user-data consent must be granted. Personalization must have a
  recorded granted or denied decision. Unknown blocks preparation. Effective
  consent time must be at or before the recorded event time. Withdrawal must be
  recorded as denied here; tracker consent changes are not an advertising-consent
  receipt or a withdrawal synchronization integration.
- Qualified event time is the first recorded qualified review in inquiry history.
  Booking event time is its revision-one audit timestamp. These are recording
  times, not a claim about when the business event actually occurred. Imported
  records without that history are blocked, never assigned the current time.
- The conservative preparation window is 30 days from captured visit to event and
  30 days from event to review. This is a local policy, not Google's conversion
  window or an assertion that a captured visit proves the time of an ad click.
- Demo data, archived inquiries, canceled bookings, spam/unqualified inquiries,
  missing history, and missing identifiers are blocked. Unreviewed inquiries can
  have booking candidates because a recorded booking is a separate outcome.
- Call attribution must have an inferred DNI window match and an explicit workspace
  opt-in. Timing is not proof of caller identity; saved numbers and delayed calls
  can refer to earlier visitors.
- Preview eligibility is recalculated on staging. Aging alone does not rewrite a
  frozen job's state. The delivery worker rechecks current eligibility,
  consent, grouping, destination, and provider requirements before every send.
- The displayed JSON is an **internal preparation record**, not a provider request.
  It excludes lead contact fields, notes, arbitrary form fields, and landing URLs.
  It contains advertising identifiers and consent references and is private.
- Cancel affects only local preparation. It does not retract a conversion in Google.
- The workflow is owner-only for hosted sessions; self-hosted operators use their
  existing workspace key. Every query, revision check, and stable ID is scoped to
  the workspace. The shared database backup already includes these tables.

## API

All endpoints use existing private workspace authentication:

- `GET /api/conversions`: settings, candidates, frozen preparations, recent history, delivery settings and the latest 200 delivery records.
- `PUT /api/conversions/settings`: customer/action IDs, inferred-call opt-in, version
  (zero on first save).
- `PUT /api/leads/{id}/conversion-consent`: `ad_user_data`, `ad_personalization`
  (`unknown`, `granted`, `denied`), `effective_at` (RFC3339 or null), `reference`,
  and version (zero on first save).
- `POST /api/conversions/stage`: candidate `kind`, `entity_key`, `fingerprint`, and
  current `job_version` passed as `version` (zero if no preparation). Stale snapshots
  fail with conflict; ineligible events fail validation. An exact retry of an
  already-prepared snapshot returns the existing job.
- `POST /api/conversions/{id}/cancel`: current preparation `version`.

- `PUT /api/conversions/delivery/settings`: `enabled` and current `version`
  (zero initially). Enabling does not queue anything.
- `POST /api/conversions/{id}/validate`: current preparation `version`;
  sends a validation-only request to Google and returns a delivery ID.
- `POST /api/conversions/deliveries/{id}/queue`: queue a validated delivery.
- `POST /api/conversions/deliveries/{id}/recheck`: poll an existing uncertain,
  failed or review-required receipt; never replay an upload.

These controls are owner-only private APIs. Public API keys and MCP connections
cannot enable or queue advertising delivery. MCP setup status reports workspace
opt-in, not proof of delivery. All tables are included in normal database backups.

## Remaining work

Microsoft delivery, automated consent synchronization, automatic event rules,
external monitoring alerts for stalled uploads and a supported automated Google
retraction path remain follow-up work. Real Google acceptance and attribution
must be verified with an eligible real event before a production cutover. Automated
tests use a local mock provider and never send synthetic Google conversions.

Provider references:
- [Sending events](https://developers.google.com/data-manager/api/devguides/events/google-ads/offline/send-events)
- [Request status](https://developers.google.com/data-manager/api/reference/rest/v1/requestStatus/retrieve)
- [Conversion adjustments](https://developers.google.com/data-manager/api/devguides/events/google-ads/conversion-adjustments)
- [Retraction limitation](https://developers.google.com/data-manager/api/devguides/events/google-ads/offline/upgrade)
