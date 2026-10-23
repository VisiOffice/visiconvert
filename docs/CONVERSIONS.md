# Google Ads conversion preparation

This shared Rust/Vite feature is available in both hosted and self-hosted builds.
It prepares qualified inquiries and bookings for review. **Google is not connected:
nothing is sent, validated, accepted, or attributed by Google in this release.**
There is no OAuth setup, provider delivery worker, retry scheduler, or retraction yet.

## Workflow

1. Open **Conversions**. Save your 10-digit Google Ads customer ID without dashes.
   Map qualification and/or bookings to distinct existing numeric conversion action
   IDs. Account access and action compatibility are not checked yet. Choose your
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
  frozen job's state. A future delivery worker must recheck current eligibility,
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

- `GET /api/conversions`: settings, candidates, frozen preparations, recent history.
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

The API rechecks the candidate under the same workspace lock used for business
mutations. No provider token, API request, or outbound network action occurs.

## Next provider slice

Connect Google credentials, discover/validate conversion actions and permissions,
map internal records to Data Manager requests, validate without sending, then add
explicit enablement and a worker with retries, request-status diagnostics, and
clear distinctions between submitted, processed, and attributed. Pin destinations
once actually sent; handle revisions, grouping/supersession, and cancellations
without double counting. Provider cancellation support must be verified before
claiming retractions. Automated consent capture/withdrawal and configurable action
windows also remain follow-up work.

Provider references checked while designing the preparation model:
- [Google Ads offline events](https://developers.google.com/data-manager/api/devguides/events/google-ads/offline)
- [Sending events](https://developers.google.com/data-manager/api/devguides/events/google-ads/offline/send-events)
- [Conversion adjustments](https://developers.google.com/data-manager/api/devguides/events/google-ads/conversion-adjustments)
