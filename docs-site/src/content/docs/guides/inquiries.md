---
title: Combine calls and forms into inquiries
description: Group repeat calls and forms from the same opportunity so they count once.
---

An **interaction** is one call or form. An **inquiry** groups the interactions about one opportunity. Every new interaction starts as its own inquiry. Nothing is combined automatically, even when the phone number or email matches.

## Combine

1. Open the inquiry in **Lead inbox** and save any qualification or note changes first.
2. Expand **Combine with another inquiry**, find the one to keep, give a reason, and choose **Combine inquiries**.

The inquiry you combine into keeps its qualification, notes and reporting source. The other one is retired, and its history is kept under **Inquiry history**.

Choose the destination carefully: its primary interaction decides which source the inquiry's revenue is reported under.

## Separate

To undo a mistaken combine, open the interaction and choose **Separate this interaction** with a reason. It becomes a new, unreviewed inquiry. Its bookings go with it.

## What changes when you group

- **Qualification and notes** belong to the inquiry. Qualifying any interaction in it, from the app, CLI, MCP or API, qualifies the whole inquiry.
- **Reports** count each inquiry once and use its primary interaction's source.
- **Lead lists** in the CLI, MCP and `/interactions` still show every call and form separately.
- **Bookings** stay attached to the interaction where they were recorded. New bookings entered on an inquiry attach to its primary interaction.

Combining and separating are only in the web app for now.

Full technical reference: [INQUIRY_GROUPING.md](https://github.com/VisiOffice/visiconvert/blob/6b76432bc1fde9686266cd5ec03449235d2a9685/docs/INQUIRY_GROUPING.md).
