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
   After mapping, click **Check saved connection** in step 1 (an agent can run the same
   check with `conversions:write`). VisiConvert then repeats the read-only check itself
   whenever it is more than 24 hours old.
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

### Owner switches

Only the workspace owner, in the Conversions page, can connect or replace the Google
login: it needs the owner's Google sign-in, and no key or agent can do it.

The advertising account, whether inferred calls are sent, Google delivery, automatic
sending and the consent policy are the owner's decisions too. Inferred calls are phone
calls matched to a website visit by timing on a DNI number, not by a click ID; they are
never sent until allowed.
The owner makes them in Conversions, or grants the separate **Sending permission**
(`conversions:send`) to an API key or MCP connection so an agent can make them after the
owner confirms in chat. `conversions:write` alone can't change them. Only the workspace
owner can grant `conversions:send`. An attestation made through a key or connection is
recorded in the name of the owner who created it (`attested_by`), and the conversion
history names the key or connection that made the change. Conversions sent to Google
can't be retracted, so revoke the key or connection once setup is finished.

`GET /api/v1/conversion-setup` and the MCP `get_conversion_setup` tool say which steps
are left and who can take them, for example *Turn on automatic sending: the owner in
Conversions, or an agent with conversions:send (set_conversion_delivery) after the owner
confirms that no other uploader sends the same Google actions.*

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

With a workspace API key (`read` to list, `conversions:write` to change actions and post
events, `conversions:send` to change the switches):

- `GET /api/v1/conversion-setup`: the checklist. `next_steps` items name the `actor`
  (`agent`, `owner_approval`, `owner` or `administrator`) and the `step`; `ready` is true
  when nothing is left. `owner_approval` steps can be done with `conversions:send` once
  the owner agrees.
- `POST /api/v1/conversion-setup/check` (`conversions:write`) `{revision, settings_version, login}`:
  the owner's **Check saved connection**, a read-only Google check of the saved account
  and mapped actions. It returns and records the same report as the owner route. Take
  `revision` from the checklist's `connection_revision`, `settings_version` from the
  checklist, and `login` from `/connections/google/accounts`. A replaced connection or
  changed destination returns 409.
- `PUT /api/v1/conversion-setup/destination` (`conversions:send`)
  `{version, customer_id, allow_inferred_calls, qualified_action_id?, booking_action_id?}`:
  the owner's **Use this advertising account** and inferred-call opt-in, saved through the
  same path. `version` is the checklist's `settings_version` (0 if none saved). The account
  must be reachable from the connected Google login and own its conversion tracking.
  Omitted reviewed-flow action IDs are kept for the same account. When you change
  accounts, pass them explicitly (`""` clears them). The checklist reports
  `customer_id` and `allow_inferred_calls`.
- `GET /api/v1/conversion-setup/google-review[?login=]` (`read`): reviews every
  non-removed conversion action in the saved Google Ads account. Each action reports `id`,
  `name`, `type`, `category`, `origin`, `status`, `primary_for_goal`, `counting`,
  `click_window_days`, value settings, `owner`, `conversions_30d`, `conversions_90d`
  (Google's all_conversions) and `last_conversion_date`, plus `mapped`, an optional `note`
  and `flags`. It also reports `goal` `{category, origin, biddable}`, the account-level
  customer conversion goal the action belongs to (`null` if Google reports none),
  `goal_shared_with`, the names of the other actions in that goal, and `bidding`. Google
  bids toward an action only when it is primary and its goal is biddable, so `bidding` is
  both, and the bidding flags below use it rather than `primary_for_goal` alone. Campaigns
  with their own campaign-level goals can differ; those are not read. Each flag has a `code`, a `severity`, a plain-language `explanation` and a
  `suggestion`:
  - `mapped` (info): VisiConvert sends to this action.
  - `active_other_uploader`: an import action VisiConvert does not send to still received
    conversions in the last 90 days. It is a warning while the action is primary, and
    info once it no longer counts for bidding.
  - `dead_upload` (warning): an import action counting for bidding with no conversions in 90 days,
    probably left over from an earlier tracking tool.
  - `call_overlap` (warning): a Google call action counting for bidding (website call, click to call,
    or a Google-hosted call action) that counts calls VisiConvert also records. A website
    call action also means Google's forwarding number can fight VisiConvert's own
    number swap.
  - Calls from ads (`AD_CALL`) are not flagged; they never reach the website.
  The result is saved. `get_conversion_setup` shows an advisory step to run the review,
  then one for the owner while warnings remain; advisory steps never block `ready`. While
  automatic sending is on, its warning names actions still flagged
  `active_other_uploader`.
- `PUT /api/v1/conversion-setup/google-actions/{id}/goal` (`conversions:send`)
  `{primary, login?}`: makes one action of the saved account primary or secondary. Only
  `primary_for_goal` is changed. Actions owned by another account return 400, and Google
  refusing the change (system actions, for example) returns 422. Google-hosted actions,
  such as clicks to call, are not sent to Google: their bidding is controlled by their
  account-level goal, so the 422 names that goal's category and origin, whether it is
  biddable, and every other action in it, for `PUT /conversion-setup/google-goal`. An
  unchanged request does not call Google. Each change is recorded in conversion history
  (`google_action_goal_changed`) under the key or MCP connection. The response is
  `changed`, the changed `action`, and the review's `summary`, `warnings`, `has_more`,
  `customer_id`, `login` and `reviewed_at`.
- `PUT /api/v1/conversion-setup/google-goal` (`conversions:send`)
  `{category, origin, biddable, login?}`: turns one customer conversion goal of the saved
  account (`customers/{id}/customerConversionGoals/{category}~{origin}`) on or off for
  bidding. Only `biddable` is sent. This changes what Smart Bidding optimizes for across
  the account, for every action in the goal; campaign-level goal overrides are never
  changed. Use `goal.category` and `goal.origin` from the review; an unknown goal returns
  404 and Google refusing returns 422. An unchanged request does not call Google. Each
  change is recorded as `google_goal_biddable_changed` with the goal, before and after,
  and the actions in it, under the key or MCP connection. The response is `changed`,
  `goal` `{category, origin, biddable, resource_name}`, `actions` (every action in the
  goal, re-classified) and the review summary and warnings.
- `GET /api/v1/conversion-delivery` (`read`): `enabled`, `auto_send`, `auto_send_at`,
  `consent_policy`, `attested_by`, `attested_at`, `version` and the `setup` checklist.
- `PUT /api/v1/conversion-delivery` (`conversions:send`)
  `{version, enabled?, auto_send?, consent_policy?, attest_no_eea_advertising?}`: omitted
  fields are unchanged and an unchanged request writes nothing. `version` is the delivery
  settings version shared with the owner's switches; a stale one returns 409. Setting
  `consent_policy: "not_required_outside_eea"` requires `attest_no_eea_advertising: true`,
  which is accepted only with that policy. While `auto_send` is on, the response includes
  a `warning` to turn off any other uploader for the same Google actions. The rules are
  the owner's: events recorded while sending was off are never sent, and turning sending
  off and on again starts a new `auto_send_at`.
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
`create_google_conversion_action`, `record_conversion_event` and `list_conversion_events`,
plus `check_google_destination`, `set_google_destination`, `get_conversion_delivery` and
`set_conversion_delivery`.
The four that change actions and events, and `check_google_destination`, require
`conversions:write`; `set_google_destination` and `set_conversion_delivery` require
`conversions:send`, and its description tells the agent to get the owner's explicit
confirmation in chat first and to state the attestation. `get_status` includes the
conversion checklist in its next steps.

`review_google_conversion_setup` (`read`), `set_google_action_goal` and
`set_google_goal_biddable` (both `conversions:send`) expose the review, the
primary/secondary switch and the account-level goal switch. The tool descriptions tell
the agent to confirm the action name and the effect on bidding with the owner first, and,
for a goal, to name every action in it and say that it changes what Smart Bidding
optimizes for across the account.

The UI uses the private, owner-checked API: `GET /api/conversions/pipeline` (actions,
policy, setup checklist and the latest 100 events), `PUT /api/conversions/actions/{key}`,
and `PUT /api/conversions/pipeline/policy` `{auto_send, consent_policy, attest, version}`,
where `version` is the delivery settings version shared with pausing and resuming
delivery and `attest: true` is required for `not_required_outside_eea`. Changes are
recorded in the conversion history.

## Limits

- Google only. Microsoft Ads (with retraction support) is planned.
- USD values only. Up to 50 actions per workspace.
