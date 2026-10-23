---
title: HTTP API
description: Authentication, errors and the core endpoints for leads, bookings, numbers and Twilio.
---

This page describes the existing UI/legacy API. The supported integration contract
is the new [public v1 API and workspace keys](/api/public-api/), currently awaiting
rollout. The UI/private API remains separate from v1.

The web app and legacy CLI use this API. Paths are relative to your server, for example `https://convert.example.com/api/leads`.

## Authentication

Send the workspace key as a bearer token:

```sh
curl -H "Authorization: Bearer $VISICONVERT_API_KEY" https://convert.example.com/api/leads
```

On a self-hosted install the key is `VISICONVERT_API_KEY`. Hosted workspaces use short-lived session tokens that start with `vc_session_`.

## Errors

Errors return a JSON body with a message:

```json
{ "error": "This record changed, or its identifier is already in use. Refresh and retry." }
```

| Status | Meaning |
|---|---|
| `400` | The request failed validation; the message says why |
| `401` | Missing or wrong key |
| `403` | The key is valid but cannot do this, such as a non-owner connecting Twilio |
| `404` | No such lead, number or booking in this workspace |
| `409` | A version or revision check failed; re-read and try again |
| `429` | Too many requests |

## Leads

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/leads` | Latest 500 individual interactions |
| `POST` | `/api/leads` | Add a lead |
| `PATCH` | `/api/leads/{id}` | Set qualification and notes |
| `GET` | `/api/leads/{id}/outcomes` | Bookings on a lead |
| `PUT` | `/api/leads/{id}/outcomes` | Record or revise a booking |

Add a lead. `external_id` is required for HTTP requests (CLI/MCP can generate one); repeating it with the same details returns the existing lead.

```json
POST /api/leads
{
  "external_id": "form-8812",
  "name": "Dana Ruiz",
  "contact": "+15125550199",
  "source": "Website",
  "campaign": "Contact form",
  "notes": ""
}
```

Qualify a lead. `version` is the lead's current version; the update is rejected with `409` if it has changed. `qualification` is `unreviewed`, `qualified`, `unqualified` or `spam`.

```json
PATCH /api/leads/{id}
{ "version": 3, "qualification": "qualified", "notes": "Two units, wants a quote" }
```

Record a booking. `external_id` is your job or invoice reference. `amount_minor` is in cents. `revision` must be 1 for a new reference, then one more than the last revision. Only `USD` is supported.

```json
PUT /api/leads/{id}/outcomes
{ "external_id": "JOB-1042", "amount_minor": 125000, "currency": "USD", "status": "booked", "revision": 1 }
```

See [Leads, qualification and bookings](/concepts/leads-and-bookings/) for why versions and revisions work this way.

## Numbers and reports

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/numbers` | Tracking numbers |
| `POST` | `/api/numbers` | Register a tracking number |
| `GET` | `/api/summary` | Leads, qualified leads, bookings and revenue by source |
| `GET` | `/api/bootstrap` | Workspace details for the web app |

```json
POST /api/numbers
{
  "label": "Spring HVAC",
  "phone_number": "+15125550142",
  "destination": "+15125550100",
  "source": "Google Ads",
  "campaign": "Spring HVAC"
}
```

## Twilio connection

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/connections/twilio` | Connection status, revision and the webhook URLs |
| `PUT` | `/api/connections/twilio` | Connect or update: `account_sid`, `auth_token`, `revision` (0 when not connected) |
| `DELETE` | `/api/connections/twilio` | Disconnect; requires the current `revision` and no tracking numbers |
| `GET` | `/api/connections/twilio/numbers` | Voice numbers in the account |

## Health

`GET /health` needs no key and returns `{"status": "ok"}`.

## More endpoints

Inquiry grouping, website tracking and dynamic number insertion have their own endpoints. They are documented on GitHub for now:

- [Inquiry grouping](https://github.com/VisiOffice/visiconvert/blob/6b76432bc1fde9686266cd5ec03449235d2a9685/docs/INQUIRY_GROUPING.md)
- [Website tracking](https://github.com/VisiOffice/visiconvert/blob/6b76432bc1fde9686266cd5ec03449235d2a9685/docs/WEBSITE_TRACKING.md)
- [Dynamic number insertion](https://github.com/VisiOffice/visiconvert/blob/6b76432bc1fde9686266cd5ec03449235d2a9685/docs/DYNAMIC_NUMBERS.md)
