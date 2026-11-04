# Pipeline conversions

Your CRM, booking tool or own code tells VisiConvert what happened to a lead and what
it was worth. VisiConvert finds the Google ad click behind that lead and sends the
conversion to Google Ads automatically. Your system keeps the value logic; VisiConvert
handles matching, click windows, deduplication, delivery and corrections.

This works the same in hosted and self-hosted workspaces and needs engine migration
0023 and a [Google connection](GOOGLE_CONNECTION.md). Microsoft Ads is not supported
yet. The reviewed, one-at-a-time flow in [conversions](CONVERSIONS.md) is unchanged and
can be used alongside it.

```text
call/form ──> VisiConvert lead (click ID from the website visit)
                 │  interaction.created webhook
                 ▼
            your system stores the VisiConvert lead ID
                 │  POST /api/v1/conversion-events  {action, external_id, value_minor}
                 ▼
            VisiConvert: match lead → click → action → window → send
                 │  Google Data Manager
                 ▼
            delivery history + conversion.delivered / conversion.failed webhooks
```

## Set up

The **Conversions** page reads top to bottom in this order:

1. **Connect Google Ads.** Sign in with Google, choose the advertising account that owns
   your conversion tracking and click **Use this advertising account**.
2. **Your actions.** Under **Conversion actions**, add one action per pipeline step and
   map each to a Google Ads import (`UPLOAD_CLICKS`) conversion action:

   | Field | Meaning |
   |---|---|
   | Key | Name your system uses, for example `lead`, `quote_sent`, `order`. Lowercase letters, digits and underscores, up to 40 |
   | Trigger | **Posted by your system** (`events`), or **Each new attributed lead** (`new_lead`): VisiConvert records it when a lead with a Google click arrives |
   | Counting | **Once per click** (one conversion per ad click, like a lead or first quote) or **Every event** (each event counts, like each order) |
   | Default value | Used for `new_lead` events and when an event has no value. Optional |
   | Google conversion action | Pick an existing eligible action from the account (preferred). Without one, events are recorded but not sent |

   If you have no suitable Google action, **Create in Google Ads** on an unmapped action
   creates a VisiConvert import action named `VisiConvert - <key> (<workspace>)` in the
   saved account: secondary (reporting only), 90-day click window, category Qualified
   lead for once-per-click actions and Purchase for every-event actions. A retry after an
   interrupted request finds the action already created instead of making another, and
   it never replaces an existing mapping.

   Key, trigger and counting cannot change once an action exists, because that would
   change what counts and could count a conversion twice. Disable an action instead.
   After mapping, click **Check saved connection** in step 1. VisiConvert then repeats
   the read-only check itself whenever it is more than 24 hours old.
3. **Advertising consent.** Choose a consent policy:
   - **Send only leads with recorded advertising consent** (default): the same rule as
     reviewed conversions.
   - **Not required: this workspace does not advertise in the EEA, UK or Switzerland.**
     You confirm this as an attestation; VisiConvert stores who confirmed it and when,
     and sends conversions without a consent signal.
4. **Automatic sending.** Turn on **Google delivery** and **automatic sending**; both must
   be on to send. The card lists anything still to do. The same day, turn off any other
   uploader for the same Google conversion actions (for example a CRM job), or each
   conversion counts twice.
5. **Results.** **Recent pipeline events** shows each event's delivery state and blocked
   reason; **Google delivery history** lists every send.

Create an API key with the **Define conversion actions and post pipeline events**
(`conversions:write`) permission for your system.

The original reviewed flow (fixed *Qualified leads* and *Bookings* actions you stage,
validate and queue one at a time) is unchanged under **Advanced: manual review** at the
bottom of the page. You don't need it for automatic conversions. Don't map the same
Google action in both flows.

### Owner-only switches

Only the workspace owner, in the Conversions page, can connect or replace the Google
login, choose the advertising account, set the consent policy and attestation, and turn
Google delivery or automatic sending on or off. API keys and MCP agents cannot, even with
every permission: those endpoints are not in the public API. `GET /api/v1/conversion-setup`
and the MCP `get_conversion_setup` tool say which of these steps is left, for example
*Owner must turn on automatic sending in Conversions.*

Events recorded while automatic sending or Google delivery is off are kept for
reporting and are **never sent later**, even if they are revised after you turn
sending on. `new_lead` events are only sent for leads that arrive after automatic
sending was turned on. This keeps a cutover from double counting.

`new_lead` conversions wait **6 hours** after the lead arrives before they are sent,
and are not sent while the inquiry is marked spam or unqualified. Google cannot take a
conversion back, so this gives you time to mark test calls and wrong numbers first.
Events your system posts (`events` actions) are sent right away.

## Post events

```http
POST /api/v1/conversion-events
Authorization: Bearer vc_key_…

{
  "action": "order",
  "external_id": "order_8812",
  "lead_id": "3f0c…",
  "value_minor": 41250,
  "occurred_at": "2026-11-04T17:02:00Z"
}
```

| Field | |
|---|---|
| `action` | Key of an action with trigger `events` |
| `external_id` | Your ID for this event, up to 150 characters |
| `lead_id` | An exact VisiConvert lead (from `interaction.created` or the API) |
| `match` | Instead of `lead_id`: `{"phone": "+16025550105"}` or `{"email": "pat@example.com"}` |
| `attribution` | With `match`: `first_click` (default) or `latest_click` |
| `value_minor` | Optional integer cents. Defaults to the action's default value |
| `occurred_at` | Optional; defaults to now. Not in the future |
| `status` | `active` (default) or `canceled` |

The response is the event with its Google delivery state:

```json
{"id":"…","action":"order","external_id":"order_8812","revision":1,
 "lead_id":"3f0c…","matched_by":"lead_id","value_minor":41250,"status":"active",
 "delivery":{"google":"queued","reason":null}}
```

`delivery.google` is one of:

| State | Meaning |
|---|---|
| `pending` | Recorded; the delivery worker will plan it shortly |
| `queued` | Frozen and waiting to send |
| `sent` | Google accepted the request; processing is not confirmed yet |
| `delivered` | Google processed it. This does not prove attribution |
| `blocked` | Not sent. `reason` says why |
| `failed`, `uncertain` | Google rejected it, or may have received it. Review in Conversions; uncertain sends are never replayed |
| `canceled` | You canceled the event |

`GET /api/v1/conversion-events?action=&external_id=&limit=&cursor=` (permission `read`)
lists events newest first with the same fields. The **Recent pipeline events** list in
Conversions shows the latest 100.

### Matching

- `lead_id` matches that lead exactly. An unknown lead returns 404.
- `match.phone` compares the last 10 digits of each call's caller number and each
  form's contact. `match.email` compares a form contact, trimmed and case-insensitive.
  Only leads with a Google click are considered. `first_click` picks the lead with
  the earliest visit (the acquisition click, so repeat orders credit the first ad);
  `latest_click` picks the most recent.
- No match is not an error: the event is stored with `matched_by: "none"` and blocked
  with *No attributed lead matched this event*. Posting it again retries the match.

Phone numbers and email addresses used for matching are not stored.

### Revisions, corrections and cancellation

- Posting the same `external_id` again with identical fields is a no-op.
- A different value or time creates the next `revision`. Omitted `value_minor` and
  `occurred_at` keep the current values.
- When a sent event gets a new value, VisiConvert sends the next revision with the same
  transaction ID, which Google treats as a restatement of the value. Only the value of
  a sent conversion can change; a changed time, lead or action is blocked.
- A revision waits while an earlier one for the same conversion is still in flight,
  failed or uncertain.
- `"status": "canceled"` stops anything not yet sent. Google Data Manager cannot retract
  a sent conversion, so a canceled event that was already sent is flagged for review in
  Conversions; adjust it in Google Ads. A canceled event is not sent again; use a new
  `external_id`.

## What gets sent

For each eligible revision VisiConvert sends one Data Manager event with the Google
click ID (gclid, gbraid or wbraid) from the lead's website visit, the event time,
the mapped action, the value in USD when there is one, and the consent signal when the
policy is per-lead. Names, contact details, notes and your `external_id` are never sent.

The click comes from the lead's form submission session, or for calls from the website
session behind a dynamic number (`inferred_window` evidence). Calls need **Allow
preparing calls with inferred website attribution** in the advanced destination
settings, as for reviewed conversions.

Transaction IDs make Google deduplicate too:

- **Once per click** uses the workspace, action and click ID. A second event for the
  same click is blocked with *Already counted for this ad click*.
- **Every event** uses the workspace, action and `external_id`.

## Blocked reasons

| Reason | What to do |
|---|---|
| No attributed lead matched this event | Check the phone or email, or post the `lead_id` |
| No Google click | The lead has no captured gclid, gbraid or wbraid. Nothing to send |
| Outside the click window | The event is later than the action's click-through window after the visit (at most 90 days). Never sent |
| Event predates the ad visit | Check `occurred_at` |
| No advertising consent recorded for this lead | Record consent, or change the consent policy if it applies |
| Recorded while automatic sending was off | Kept for reporting; never sent |
| Automatic sending is off | Sent after you turn automatic sending back on |
| No Google conversion action is mapped / Action is disabled | Map or enable the action |
| Waiting for the Google destination check | VisiConvert refreshes the check automatically |
| The mapped Google action is not an enabled import action… | Fix the action in Google Ads or the mapping |
| Already counted for this ad click | Expected for once-per-click actions |
| The inquiry is marked spam | Change the review if it is not spam |
| The inquiry is marked unqualified | `new_lead` actions only. Change the review if it is a real lead |
| Held for 6 hours so the lead can be reviewed before sending | `new_lead` actions only. It sends automatically when the hold ends |
| Inferred call attribution requires explicit opt-in | Turn it on in the advanced destination settings |

Temporary reasons are rechecked automatically within 15 minutes (hourly while automatic
sending is off).

## Delivery

The existing delivery worker runs a planning pass every 10 seconds. It records
`new_lead` events, refreshes an expired destination check, and freezes each eligible
revision as a queued delivery. Each send rechecks the event revision, action mapping,
policy, consent and connection first. Sends then follow the same rules as reviewed
delivery: receipts are polled, retries are bounded, and an uncertain send is never
repeated. Pausing Google delivery or turning off automatic sending holds queued
automatic sends until you resume.

If an automatic destination check fails, sending pauses: the delivery stays queued,
**Recent pipeline events** shows why, and a `conversion.failed` webhook is sent once.
Pipeline deliveries send the existing `conversion.delivered` and `conversion.failed`
[webhooks](WEBHOOKS.md); their data includes a `pipeline_event` object with the event
`id`, `action`, `external_id`, `revision` and `value_minor`.

## API and MCP

With a workspace API key (`read` to list, `conversions:write` to change):

- `GET /api/v1/conversion-setup`: the checklist. `next_steps` items name the `actor`
  (`owner`, `agent` or `administrator`) and the `step`; `ready` is true when nothing is left.
- `GET /api/v1/connections/google/accounts[?login=]` and
  `GET /api/v1/connections/google/actions?login=&customer=`: read-only discovery for the
  connected Google login.
- `GET /api/v1/conversion-actions`, `PUT /api/v1/conversion-actions/{key}`: list, create
  (`version: 0`) or update actions at their current version.
- `POST /api/v1/conversion-actions/{key}/google-action` `{version, login, category?}`:
  create a VisiConvert action in Google for an unmapped action. Prefer mapping an existing
  one with `google_action_id`.
- `POST`/`GET /api/v1/conversion-events`: as above.

The MCP server (local `visiconvert mcp` and hosted remote MCP) exposes the same operations:
`get_conversion_setup`, `list_google_accounts`, `list_google_conversion_actions`,
`list_conversion_actions`, `create_conversion_action`, `update_conversion_action`,
`create_google_conversion_action`, `record_conversion_event` and `list_conversion_events`.
The four that change data require `conversions:write`. `get_status` includes the
conversion checklist in its next steps.

The UI uses the private, owner-checked API: `GET /api/conversions/pipeline` (actions,
policy, setup checklist and the latest 100 events), `PUT /api/conversions/actions/{key}`,
and `PUT /api/conversions/pipeline/policy` `{auto_send, consent_policy, attest, version}`,
where `version` is the delivery settings version shared with pausing and resuming
delivery and `attest: true` is required for `not_required_outside_eea`. Changes are
recorded in the conversion history.

## Limits

- Google only. Microsoft Ads (with retraction support) is planned.
- USD values only. Up to 50 actions per workspace.
