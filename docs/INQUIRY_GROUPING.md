# Group calls and forms into inquiries

Available in the shared Rust application for both self-hosted and hosted workspaces. Rails billing is not required.

An **interaction** is an individual call or form submission. An **inquiry** groups interactions about one opportunity. A booking remains attached to the interaction where it was recorded. This feature does not create a customer/contact directory or automatically merge people with matching phone numbers or email addresses.

## Operator workflow

1. Open an inquiry in **Lead inbox**. Its interaction timeline shows original contact details, sources, call status, form evidence, and booked values.
2. Save any qualification or note edits before changing the group.
3. Expand **Combine with another inquiry**, search for the destination, and review its source and qualification. Supply a reason and select **Combine inquiries**.
4. The destination retains its identity, reporting source, qualification, and notes. The source inquiry is retired; its review snapshot and history remain available under **Inquiry history**. All its interactions move together.
5. To correct an association, choose **Separate this interaction** and supply a reason. This creates a new, unreviewed inquiry with empty inquiry notes. Original interaction notes and evidence remain available. Review the resulting inquiries before using their qualification counts.

Combining requires an explicit operator decision. Matching contact information may help with searching but does not establish identity or indicate that two requests concern the same opportunity.

## Reporting and bookings

The inbox and source report count inquiries once, even when they contain multiple calls or forms. Their qualification belongs to the inquiry. Searching the inbox also matches the names, contacts, sources, and campaigns of its member interactions.

A primary interaction supplies each inquiry's displayed name/contact and reporting source. Combining keeps the destination's primary interaction: choose the destination deliberately. Every member's original source remains in its timeline. If the primary interaction is separated, the original inquiry uses its oldest remaining interaction as the reporting source. These choices can move revenue between source rows without changing the workspace's total revenue.

Bookings retain their original interaction attachment and external ID. Combining aggregates their current booked values once. Splitting moves only bookings attached to the separated interaction. New bookings entered on an inquiry attach to its primary interaction. Distinct booking IDs remain distinct even when they represent a real-world duplicate; cancel or revise the duplicate explicitly. Booking IDs remain unique across the workspace, and canceled outcomes do not contribute to booked revenue.

## History, conflicts, and limits

Combines, splits, review edits, and booking changes record an actor and event. Combine/split events retain the reason, previous review snapshot, and affected IDs. Self-host operator actions identify the workspace operator; hosted actions identify the authenticated hosted user. Tokens and credentials are never recorded in this history.

Concurrent changes require a fresh inquiry version. Combining checks both source and destination versions. Group versions advance beyond previous membership versions so an old interaction-based edit cannot overwrite a newly combined or separated inquiry's review. After a conflict, reload and review the current state before retrying.

The beta inbox/destination selector returns the latest 500 active inquiries; a group supports at most 100 interactions. History shows the latest 100 events per inquiry, with links to earlier inquiry history. Demo and live interactions cannot be combined. Automatic grouping, contact entities, reassignment of a booking to another interaction, and a general-purpose undo operation are not included.

## API compatibility

All routes below are authenticated and workspace-scoped:

| Route | Behavior |
| --- | --- |
| `GET /api/inquiries` | Latest active inquiries, including `primary_lead_id`, `interaction_count`, and `search_text` |
| `GET /api/inquiries/{id}` | Snapshot containing `inquiry`, `interactions`, and `history` |
| `PATCH /api/inquiries/{id}` | Update `{qualification, notes, version}` |
| `POST /api/inquiries/{id}/combine` | Move all members to `{target_id, version, target_version, reason}` |
| `POST /api/inquiries/{id}/split` | Separate `{interaction_id, version, reason}` |
| `GET /api/inquiries/{id}/history` | History, including for retired inquiries |
| `GET /api/inquiries/{id}/outcomes` | Group outcomes with original `lead_id` attachment |
| `PUT /api/inquiries/{id}/outcomes` | Existing booking schema; new bookings attach to the primary interaction |

Combine and split return the resulting inquiry ID as `{id}`. A retired inquiry no longer appears in the active list or detail endpoint; its history remains readable by the same workspace.

Legacy `/api/leads` continues to return individual interactions, original sources, and per-interaction booked values. It adds `inquiry_id`; its qualification, notes, and version reflect the current inquiry. Legacy review writes resolve the interaction's current inquiry and check its version. Legacy outcome reads remain per interaction; writes retain their original booking attachment and must belong to the same current inquiry. Existing call callbacks, form submission IDs, and website evidence URLs still use stable interaction IDs. `/api/summary` now counts inquiries rather than individual interactions (the historical response field remains named `leads`). The existing CLI/MCP lead list is therefore an interaction list; dedicated grouping commands are future work.

## Installation and verification

Migration `0006_inquiry_grouping.sql` creates one inquiry per existing interaction, retaining its ID, review, timestamps, and booking attachment. New intake paths create one inquiry atomically, including Twilio callbacks and website forms. Retried intake does not create extra inquiry rows. No new credential or configuration is required. Use the normal backup-before-upgrade procedure; the migration does not attempt to infer historical groups.

Disposable database tests cover migration from populated pre-grouping tables, hosted/operator isolation, concurrent combines, stale edits, source changes, split booking ownership, callbacks, form retries, and website evidence. The browser verifier exercises combining, reviewing, history, splitting, source totals, and desktop/mobile layout. Live business acceptance remains a separate step.
