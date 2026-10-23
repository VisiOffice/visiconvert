---
title: Leads, qualification and bookings
description: How leads, qualification, bookings and revisions work, and why some updates are rejected.
---

## Sources and campaigns

A **source** is where the inquiry came from, such as Google Ads, Facebook or a yard sign. A **campaign** is a narrower label within it. Each tracking number has one of each, so every call it receives is labeled automatically. Leads added by form or API carry their own source and campaign.

## Leads and qualification

Every call or form submission is a **lead**. A new lead is `unreviewed`. You then mark it:

- `qualified`: a real prospect for your business.
- `unqualified`: a real person, but not a fit (wrong area, wrong service).
- `spam`: robocalls, sales pitches and junk.

Reports count qualified leads separately, so spam does not make a source look better than it is.

### Version checks

Qualification, notes, and `version` are shared by the inquiry. Updating any member interaction updates the whole inquiry. Each lead response carries that shared `version`. An update must send the version it read, and the server rejects it with `409 Conflict` if someone else changed the lead in the meantime. This stops two people, or a person and an agent, from silently overwriting each other. The CLI reads the current version for you and keeps existing notes unless you replace them.

## Bookings

A **booking** records a job that came from a lead. It has:

- A **reference**: your job or invoice number, such as `JOB-1042`.
- An **amount** in US dollars, stored as exact cents.
- A **status**: `booked` or `canceled`.

### Revisions

A reference identifies one booking. To change its amount or cancel it, you send the same reference again with the next **revision** number: 1 for the first record, then 2, 3 and so on. Anything else is rejected, which makes retries safe: sending the same revision twice cannot double-count revenue.

A reference is unique within a workspace and stays attached to its original interaction. It can be revised through another member of the same inquiry using the correct revision; another inquiry cannot take it over. CLI/MCP currently look up bookings on the original interaction.

From the command line, the CLI works out the revision for you:

```sh
visiconvert leads outcome 9ec1ff6e --ref JOB-1042 --booked 1250.00
visiconvert leads outcome 9ec1ff6e --ref JOB-1042 --booked 1400.00   # revises it
visiconvert leads outcome 9ec1ff6e --ref JOB-1042 --canceled
```

## Duplicate leads

Leads added through the API can carry an `external_id`, such as a form submission ID. Sending the same lead again with the same ID returns the existing lead instead of creating a second one.

Repeat calls and form fills can be explicitly grouped into one inquiry in the web app. There is no automatic identity matching. Reports count active inquiries and use their primary source, while interaction listings show every call/form separately. Separating an interaction keeps its original bookings.
