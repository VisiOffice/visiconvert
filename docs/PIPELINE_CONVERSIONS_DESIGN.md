# Pipeline conversions (design)

Status: phase 1 (Google, automatic) implemented, 2026-11-04. User documentation:
[PIPELINE_CONVERSIONS.md](PIPELINE_CONVERSIONS.md). Phases 2 and 3 below remain design.

Phase 1 differs from this design in these points:

- `new_lead` events use `matched_by: "new_lead"` and are only sent for leads that
  arrive after automatic sending was turned on.
- Events recorded while automatic sending or delivery is off are never sent later, even
  when revised, so a cutover cannot double count with an older uploader.
- Revisions of a sent conversion may change only the value; a changed time, click or
  action is blocked rather than restated.
- A second once-per-click event for the same click is blocked locally as well as
  deduplicated by Google's transaction ID.
- Calls still require the existing inferred-call opt-in.
- `new_lead` conversions are held 6 hours (WhatConverts queues each conversion about
  6 hours too) and are not sent while the inquiry is spam or unqualified.
- Delivery states add `pending`, `delivered`, `failed`, `uncertain` and `canceled` to
  `queued`, `sent` and `blocked`.
- MCP goes further than listing: with `conversions:write` an agent can create, map and
  update actions, create a Google action for one, and record events. Owner-only switches
  (Google OAuth, advertising account, consent policy, delivery, automatic sending) are not
  reachable by keys or MCP; a setup checklist names which are left. A follow-up added
  the owner-granted `conversions:send` scope, which reaches delivery, automatic sending
  and the consent policy (attested in the owner's name) through
  `PUT /api/v1/conversion-delivery` and `set_conversion_delivery`. Google OAuth and the
  advertising account stay owner-only.
- The Conversions page is pipeline-first; the reviewed qualified/booking flow is kept
  unchanged under "Advanced: manual review".

## Goal

A business's own system (CRM, booking tool, or a person in the VisiConvert UI)
tells VisiConvert what happened to a lead and what it was worth. VisiConvert finds
the ad click behind that lead and sends the conversion to Google Ads, and later
Microsoft Ads, automatically. Value logic stays with the business; VisiConvert
owns matching, click windows, deduplication, delivery and corrections.

First customer: Quarry, replacing its own `ConversionProgressionService` and
`PushConversionToGoogleAdsJob`.

```text
call/form ──> VisiConvert lead (click ID from the website visit)
                 │  interaction.created webhook (exists today)
                 ▼
            Quarry stores the VisiConvert lead ID
                 │  POST /v1/conversion-events  {action, external_id, value, time}
                 ▼
            VisiConvert: match lead → click → action → window → send
                 │  Google Data Manager (now), Microsoft Ads (phase 2)
                 ▼
            delivery history + conversion.delivered / conversion.failed webhooks
```

## What exists, and what stays

The reviewed flow in [CONVERSIONS.md](CONVERSIONS.md) stays as is: fixed
`qualified` and `booking` kinds, staged by hand, validated, queued one at a time.
Pipeline conversions are a separate automatic path that reuses its Google
connection, Data Manager request builder, durable delivery worker, receipts,
polling and webhooks. Nothing already prepared is moved or requeued.

## Model

### Actions (workspace-defined)

`conversion_actions`, one row per workspace and `key`:

| Field | Meaning |
| --- | --- |
| `key` | Slug the API uses, e.g. `lead`, `quote_sent`, `order`. `^[a-z][a-z0-9_]{0,39}$` |
| `name` | Display name |
| `trigger` | `events` (posted by the business) or `new_lead` (VisiConvert creates it when an attributed lead arrives) |
| `default_value_minor` | Value used by `new_lead`, or when an event omits one. Null means no value |
| `counting` | `once_per_click` (one per ad click, like Quarry's lead) or `every` (each event counts, like orders) |
| `google_action_id` | Mapped Google conversion action, or null |
| `enabled`, `version` | Compare-and-swap like other settings |

Quarry's mapping:

| key | trigger | value | counting |
| --- | --- | --- | --- |
| `lead` | `new_lead` | none | once per click |
| `quote_sent` | events | $20 (Quarry sends 2000) | once per click |
| `order` | events | margin, or 6 months margin if recurring | every |

### Events

`pipeline_events`, unique on `(workspace, action_key, external_id)`:
`lead_id`, `matched_by`, `value_minor`, `currency` (USD), `occurred_at`,
`revision`, `status` (`active`/`canceled`), timestamps. Posting the same
`external_id` again with a different value or time creates the next revision;
an identical repost is a no-op. Cancel sets `status=canceled`.

### Policy

Added to `conversion_delivery_settings`:

- `auto_send` (default false). Pipeline events are always recorded; they are only
  sent when this and the existing `enabled` flag are on.
- `consent_policy`: `per_lead` (today's rule: recorded advertising consent) or
  `not_required_outside_eea`. The second is an owner attestation that the
  workspace does not advertise to the EEA, UK or Switzerland. It is stored with
  who attested and when, and sent to Google with no consent block. Quarry uses it.

## API

Public API, new scope `conversions:write` (shown in API Keys with the other
write permissions; `read` can list):

```http
POST /v1/conversion-events
{
  "action": "order",
  "external_id": "order_8812",
  "lead_id": "3f0c…",                    // or "match": {"phone": "+16025550105"} / {"email": "…"}
  "attribution": "first_click",          // or "latest_click"; default first_click
  "value_minor": 41250,                  // optional; integer cents
  "occurred_at": "2026-11-04T17:02:00Z", // optional; default now; not in the future
  "status": "active"                     // "canceled" to withdraw
}
→ 200 {"id", "revision", "lead_id", "matched_by", "delivery": {"google": "queued" | "sent" | "blocked", "reason"}}

GET /v1/conversion-events?action=&external_id=&cursor=
```

Matching:

- `lead_id`: an exact VisiConvert lead.
- `match.phone` / `match.email`: leads with that caller number or form contact,
  restricted to leads that have a click ID. `first_click` picks the earliest (the
  acquisition click, which is how Quarry credits repeat orders); `latest_click`
  picks the most recent.
- No match is not an error. The event is stored with `matched_by=none` and
  reported as `blocked: no attributed lead`, so the business can see why.

Owner-only private APIs and a Conversions UI section manage actions and policy.
MCP gets read-only listing of actions and event delivery state.

## Delivery

A planner pass runs in the existing delivery worker loop. For each active event
revision whose action is mapped and whose workspace has `auto_send` on:

1. Resolve the click: gclid/gbraid/wbraid from the lead's website session, using
   the same joins as today (form submission, or DNI `inferred_window` call
   evidence). No click ID means blocked: no Google click.
2. Check eligibility: not demo or spam; consent per policy; event at or after the
   visit; event within the action's click window (from the last destination
   check, at most 90 days). Outside the window it is recorded as
   blocked: outside click window, never sent.
3. Transaction ID: `once_per_click` hashes `(workspace, action, click ID)`, so a
   second quote for the same click deduplicates at Google too. `every` hashes
   `(workspace, action, external_id)`.
4. Freeze a snapshot in `conversion_outbox` (kind `pipeline`, excluded from the
   workspace-wide invalidation trigger) and insert a `queued` delivery. The
   existing worker sends it, records the request ID and polls.

Revisions: when a delivered event gets a new value, the planner queues the next
revision with the same transaction ID and the new value. Google treats that as a
restatement and replaces the value. A revision waits while an earlier one is
in flight, failed or uncertain, as it does today.

Cancellation stops unsent work. Google has no retraction in Data Manager, so an
already-sent cancel is flagged for review (Microsoft phase 2 can retract).

Destination checks: today a person must re-check the Google destination every
24 hours. The worker will re-run that read-only check itself when it is older
than 24 hours, before sending. If the check fails, sending pauses and the
Conversions page and `conversion.failed` webhook say why.

Uncertain sends never replay, exactly as today.

## Phases

1. **Google, automatic** (this design): actions, events API with `conversions:write`,
   policy, planner, worker destination refresh, Conversions UI for actions and
   event status, docs, tests against the local mock provider.
2. **Microsoft Ads**: OAuth, developer token (provider application required),
   `ApplyOfflineConversions` with msclkid and goal name, `ApplyOfflineConversionAdjustments`
   for restate and retract, per-action `microsoft_goal_name`. Implemented in migration
   0027 with separate Microsoft tables; see [Microsoft connection](MICROSOFT_CONNECTION.md).
3. **No-CRM UI**: mark a lead quoted/sold with a value from the inbox.

## Quarry cutover

1. Connect Google in VisiConvert and map `lead`, `quote_sent` and `order` to
   Quarry's existing Google conversion actions, so history and bidding continue.
2. Quarry: store the VisiConvert lead ID from `interaction.created`; post
   `quote_sent` and `order` events (the value logic stays in Quarry); post order
   updates with the same `external_id`.
3. Same day as auto-send is enabled: turn off `PushConversionToGoogleAdsJob` for
   those actions, or every conversion counts twice.
4. After a verified real conversion in Google: remove the old uploader from Quarry.
