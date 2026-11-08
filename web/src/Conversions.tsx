import { useEffect, useState, type FormEvent, type KeyboardEvent } from "react";
import { type Client, money, label } from "./api";
import { GoogleConnection, type Discovery } from "./GoogleConnection";
import { ReviewedGoogleActions } from "./ReviewedGoogleActions";
import { PipelineConversions, type Setup } from "./PipelineConversions";
import { MicrosoftConnection } from "./MicrosoftConnection";
import { MetaConnection } from "./MetaConnection";

export type Settings = { customer_id: string; qualified_action_id: string; booking_action_id: string; allow_inferred_calls: boolean; version: number };
type Decision = "unknown" | "granted" | "denied";
type Consent = { ad_user_data: Decision; ad_personalization: Decision; effective_at: string | null; reference: string; version: number };
type Snapshot = { lead_id: string; inquiry_id: string; channel: string; source: string; event_at: string | null; visit_at: string | null; amount_minor: number | null; confidence: string; consent: Consent; transaction_id: string };
type Candidate = { display_name: string; kind: string; entity_key: string; snapshot: Snapshot; fingerprint: string; eligible: boolean; reasons: string[]; job_version: number; job_state: string | null };
type Job = { id: string; kind: string; entity_key: string; state: string; version: number; transaction_id: string; snapshot: Snapshot };
type Delivery = { id: string; outbox_id: string; outbox_version: number; state: string; review_required: boolean; request_id: string | null; diagnostic: string | null; warnings: boolean; attempts: number; polls: number };
type Preview = { delivery_settings: { enabled: boolean; version: number }; deliveries: Delivery[]; settings: Settings; candidates: Candidate[]; jobs: Job[]; limit: number; history: { id: number; kind: string; actor: string; created_at: string }[] };
const identity = (c: { kind: string; entity_key: string }) => `${c.kind}:${c.entity_key}`;
export type Tab = "overview" | "actions" | "google" | "microsoft" | "meta" | "events" | "advanced";
export type Panel = (t: Tab) => { id: string; role: "tabpanel"; "aria-labelledby": string; hidden: boolean; className: string };
const TABS: [Tab, string][] = [["overview", "Overview"], ["actions", "Actions"], ["google", "Google Ads"], ["microsoft", "Microsoft Ads"], ["meta", "Meta Ads"], ["events", "Events"], ["advanced", "Advanced"]];
const date = (s: string | null) => s ? new Date(s).toLocaleString() : "Not recorded";
export function Conversions({ api, tab: routeTab, onTab }: { api: Client; tab: string | null; onTab: (t: Tab) => void }) {
  const [data, setData] = useState<Preview | null>(null);
  const [selected, setSelected] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  const [discovery, setDiscovery] = useState<Discovery | null>(null);
  // Bumped when the Microsoft section changes actions or the checklist.
  const [microsoftKey, setMicrosoftKey] = useState(0);
  const tab: Tab = TABS.some(([t]) => t === routeTab) ? routeTab as Tab : "overview";
  const setTab = onTab;
  const [setup, setSetup] = useState<Setup | null>(null);
  async function refresh() { setData(await api<Preview>("/conversions")); }
  useEffect(() => { let active = true; api<Preview>("/conversions").then(d => { if (active) setData(d); }).catch(e => { if (active) setError(e.message); }); return () => { active = false; }; }, [api]);
  async function mutate(path: string, body: unknown, message: string, method?: string) {
    setBusy(true); setError(""); setNotice("");
    try { await api(path, body, method); await refresh(); setNotice(message); }
    catch (e) { setError((e as Error).message); await refresh().catch(() => {}); }
    finally { setBusy(false); }
  }
  const candidate = data?.candidates.find(c => identity(c) === selected) ?? data?.candidates[0];
  const panel = (t: Tab) => ({ id: `conversions-panel-${t}`, role: "tabpanel", "aria-labelledby": `conversions-tab-${t}`, hidden: tab !== t, className: "conversion-panel" } as const);
  const manualInUse = !!data && !!(data.settings.qualified_action_id || data.settings.booking_action_id || data.jobs.length);
  const dot = (t: Tab) => {
    if (!setup) return null;
    if (t === "google") return <i className={`tab-dot ${setup.ready ? "ok" : "todo"}`} aria-hidden="true" />;
    if (t === "microsoft") { const m = setup.microsoft; return m?.configured ? <i className={`tab-dot ${m.ready ? "ok" : m.connected ? "todo" : "off"}`} aria-hidden="true" /> : null; }
    if (t === "meta") { const m = setup.meta; return m && m.configured !== false ? <i className={`tab-dot ${m.ready ? "ok" : m.connected ? "todo" : "off"}`} aria-hidden="true" /> : null; }
    return null;
  };
  function choose(t: Tab) { setTab(t); setNotice(""); }
  function keys(e: KeyboardEvent<HTMLDivElement>) {
    const i = TABS.findIndex(([t]) => t === tab);
    const next = e.key === "ArrowRight" ? (i + 1) % TABS.length : e.key === "ArrowLeft" ? (i - 1 + TABS.length) % TABS.length : e.key === "Home" ? 0 : e.key === "End" ? TABS.length - 1 : -1;
    if (next < 0) return;
    e.preventDefault(); setTab(TABS[next][0]);
    document.getElementById(`conversions-tab-${TABS[next][0]}`)?.focus();
  }
  return <div className="conversion-layout">
    <div className="conversion-tabs" role="tablist" aria-label="Conversion settings" onKeyDown={keys}>
      {TABS.map(([t, name]) => <button key={t} id={`conversions-tab-${t}`} role="tab" type="button" aria-selected={tab === t} aria-controls={`conversions-panel-${t}`} tabIndex={tab === t ? 0 : -1} className={`conversion-tab ${tab === t ? "on" : ""}`} onClick={() => choose(t)}>{name}{dot(t)}{t === "advanced" && manualInUse && <span className="tab-note">in use</span>}</button>)}
    </div>
    {error && <p className="error" role="alert">{error}</p>}
    {!data ? <p>Loading conversions…</p> : <>
      <PipelineConversions api={api} revision={data.delivery_settings.version} settingsVersion={data.settings.version} refreshKey={microsoftKey} discovery={discovery?.customer === data.settings.customer_id ? discovery : null} onChange={refresh} onSetup={setSetup} tab={tab} panel={panel} open={choose} />
      <div {...panel("google")}>
        <GoogleConnection api={api} settings={data.settings} onChange={refresh} onDiscovery={setDiscovery} />
      </div>
      <div {...panel("microsoft")}>
        <MicrosoftConnection api={api} revision={data.delivery_settings.version} onChange={async () => { setMicrosoftKey(k => k + 1); await refresh(); }} />
      </div>
      <div {...panel("meta")}>
        <MetaConnection api={api} revision={data.delivery_settings.version} onChange={async () => { setMicrosoftKey(k => k + 1); await refresh(); }} />
      </div>
      <div {...panel("advanced")}>
      {notice && <p className="notice" role="status">{notice}</p>}
      <section className="card site-panel" aria-label="Advanced: manual review">
        <div><span className="eyebrow">ADVANCED</span><h2>Manual review{manualInUse ? " · in use" : ""}</h2></div>
        <p className="muted">The original reviewed flow: fixed qualified-lead and booking actions that you stage, validate with Google and queue one conversion at a time. You don’t need it for automatic conversions. Don’t map the same Google action in both flows, or conversions count twice.</p>
      </section>
        <ReviewedGoogleActions api={api} discovery={discovery?.customer === data.settings.customer_id ? discovery : null} settings={data.settings} onChange={refresh} />
      <section className="card site-panel" aria-label="Google delivery controls">
        <div className="section-heading"><div><span className="eyebrow">GOOGLE DELIVERY</span><h2>{data.delivery_settings.enabled ? "Delivery enabled" : "Delivery paused"}</h2></div><button className="secondary" disabled={busy} onClick={() => void mutate("/conversions/delivery/settings", { enabled: !data.delivery_settings.enabled, version: data.delivery_settings.version }, data.delivery_settings.enabled ? "Pending sends paused. Existing receipts can still be checked." : "Delivery enabled. Only explicitly queued events will be sent.", "PUT")}>{data.delivery_settings.enabled ? "Pause Google delivery" : "Enable reviewed delivery"}</button></div>
        <p className="muted">Use one uploader per conversion action. Turn off any overlapping CRM uploads before queuing here. Changes to consent or bookings stop pending sends for review. Google Data Manager cannot retract an already sent conversion; canceled bookings and withdrawn consent require manual correction in Google.</p>
      </section>
      <Mapping key={data.settings.version} value={data.settings} busy={busy} save={s => mutate("/conversions/settings", s, "Destination saved. Prepared events require review.", "PUT")} />
      <section className="card site-panel">
        <div className="section-heading"><div><span className="eyebrow">GOOGLE ADS</span><h2>Conversion preview</h2></div><button className="secondary" disabled={busy} onClick={() => { setError(""); void refresh().catch(e => setError(e.message)); }}>Refresh conversions</button></div>
        <p className="muted">Latest {data.limit} qualified inquiries and bookings. Qualification uses the inquiry’s primary interaction; bookings keep their original interaction. Event time is the first recorded qualification or booking, which may differ from when it happened in your business.</p>
        {!candidate ? <p>No candidates yet. Qualify an inquiry or record a booking in the lead inbox.</p> : <>
          <div className="form-field"><label htmlFor="conversion-select">Conversion event</label><select id="conversion-select" value={identity(candidate)} onChange={e => setSelected(e.target.value)}>{data.candidates.map(c => <option key={identity(c)} value={identity(c)}>{label(c.kind)} · {c.display_name} · {c.kind === "booking" ? c.entity_key : date(c.snapshot.event_at)} · {c.eligible ? "Ready for preparation" : "Needs evidence"}</option>)}</select></div>
          <dl className="site-diagnostics"><div><dt>Source / confidence</dt><dd>{candidate.snapshot.source} / {candidate.snapshot.confidence}</dd></div><div><dt>Recorded event time</dt><dd>{date(candidate.snapshot.event_at)}</dd></div><div><dt>Value</dt><dd>{candidate.snapshot.amount_minor === null ? "No monetary value" : money(candidate.snapshot.amount_minor)}</dd></div></dl>
          <p className="muted">Browser identifiers describe a captured visit, not verified ad attribution. Call attribution is inferred from a number assignment window and may match an earlier visitor. Preparation requires a visit-to-event interval and event age of at most 30 days; Google’s action-specific rules still need validation.</p>
          {candidate.reasons.length > 0 ? <ul className="conversion-reasons">{candidate.reasons.map(r => <li key={r}>{r}</li>)}</ul> : <p className="notice">Ready for preparation. Google acceptance and attribution have not been checked.</p>}
          <ConsentForm key={`${candidate.snapshot.lead_id}:${candidate.snapshot.consent.version}`} value={candidate.snapshot.consent} busy={busy} save={c => mutate(`/leads/${candidate.snapshot.lead_id}/conversion-consent`, c, "Consent evidence recorded.", "PUT")} />
          <details className="conversion-snapshot"><summary>Review the prepared data</summary><p>Internal preparation record; not a Google API request. Includes click identifiers and consent references. Do not share publicly.</p><pre>{JSON.stringify(candidate.snapshot, null, 2)}</pre></details>
          <button className="primary" disabled={busy || !candidate.eligible || candidate.job_state === "awaiting_connection"} onClick={() => void mutate("/conversions/stage", { kind: candidate.kind, entity_key: candidate.entity_key, fingerprint: candidate.fingerprint, version: candidate.job_version }, "Conversion staged locally. Nothing was sent to Google.")}>{candidate.job_state === "awaiting_connection" ? "Already staged" : "Stage reviewed conversion"}</button>
        </>}
      </section>
      <section className="card site-panel">
        <h2>Prepared conversions</h2>
        <p className="muted">Latest {data.limit} preparations. Booking, grouping, consent, or destination changes require another review. For now, a change anywhere in this workspace conservatively marks all staged events for review. Cancel removes a local preparation from consideration; it does not retract a Google conversion.</p>
        {!data.jobs.length ? <p>No conversions staged yet.</p> : <div className="conversion-jobs">{data.jobs.map(j => <article className="conversion-job" key={j.id} aria-label={`Prepared ${j.kind} ${j.entity_key}`}>
          <strong>{label(j.kind)} · {j.entity_key}</strong><span className="badge">{label(j.state)}</span>
          <small>Stable transaction ID: <code>{j.transaction_id}</code></small>
          <details className="conversion-snapshot"><summary>Saved snapshot</summary><pre>{JSON.stringify(j.snapshot, null, 2)}</pre></details>
          {j.state === "awaiting_connection" && !data.deliveries.some(d => d.outbox_id === j.id && d.outbox_version === j.version && !["needs_review", "validated"].includes(d.state)) && <button className="secondary" disabled={busy} onClick={() => void mutate(`/conversions/${j.id}/validate`, { version: j.version }, "Google event validation passed. No conversion was recorded. Review and queue when ready.")}>Validate event with Google</button>}
          {j.state !== "canceled" && <button className="secondary" disabled={busy} onClick={() => void mutate(`/conversions/${j.id}/cancel`, { version: j.version }, "Local preparation canceled.")}>Cancel preparation</button>}
        </article>)}</div>}
      </section>
      <details className="card site-panel"><summary>Preparation history (latest 100)</summary><ul className="inquiry-history">{data.history.map(h => <li key={h.id}>{label(h.kind)} · {h.actor} · {date(h.created_at)}</li>)}</ul></details>
      <section className="card site-panel" aria-label="Manual delivery history"><h2>Manual delivery history</h2>
        <p className="muted">Processed means Google processed this request; it does not prove ad attribution or inclusion in bidding reports. Warnings and uncertain outcomes need review using the saved request ID. An uncertain send is never automatically submitted again.</p>
        {!data.deliveries.length ? <p>No events validated or sent yet.</p> : <div className="conversion-jobs">{data.deliveries.map(d => <article className="conversion-job" key={d.id}>
          <strong>{label(d.state)}{d.review_required ? " · Review required" : ""}</strong>
          <small>Preparation {d.outbox_id} · revision {d.outbox_version} · {d.attempts} delivery attempts</small>
          {d.request_id && <small>Google request ID: <code>{d.request_id}</code></small>}
          {d.diagnostic && <p>{d.diagnostic}</p>}
          {d.warnings && <p className="notice">Google reported warnings. Inspect the request in Google diagnostics.</p>}
          {d.state === "validated" && !d.review_required && <button className="primary" disabled={busy || !data.delivery_settings.enabled} onClick={() => void mutate(`/conversions/deliveries/${d.id}/queue`, {}, "Delivery queued. Refresh to check Google processing status.")}>Queue conversion for Google</button>}
          {d.request_id && ["uncertain", "needs_review", "failed"].includes(d.state) && <button className="secondary" disabled={busy} onClick={() => void mutate(`/conversions/deliveries/${d.id}/recheck`, {}, "Receipt check queued. The event will not be uploaded again.")}>Recheck Google receipt</button>}
        </article>)}</div>}
      </section>
      </div>
    </>}
  </div>;
}
function Mapping({ value, busy, save }: { value: Settings; busy: boolean; save: (s: Settings) => Promise<void> }) {
  const [draft, setDraft] = useState(value);
  return <details className="card site-panel"><summary>Advanced destination settings</summary><h2>Enter account and action IDs</h2><p className="muted">Use your Google Ads customer ID without dashes and existing import conversion action IDs. Map only the event types you want; leave both action IDs blank if you only use automatic conversion actions. Saving IDs does not verify them; use the Google connection’s validation control to check the saved destination.</p><form className="site-form" onSubmit={e => { e.preventDefault(); void save(draft); }}>
    <div className="form-field"><label htmlFor="ads-customer">Google Ads customer ID</label><input id="ads-customer" required inputMode="numeric" pattern="[0-9]{10}" maxLength={10} value={draft.customer_id} onChange={e => setDraft({ ...draft, customer_id: e.target.value })} /></div>
    <div className="form-field"><label htmlFor="ads-qualified">Qualified inquiry action ID</label><input id="ads-qualified" inputMode="numeric" pattern="[0-9]{1,20}" maxLength={20} value={draft.qualified_action_id} onChange={e => setDraft({ ...draft, qualified_action_id: e.target.value })} /></div>
    <div className="form-field"><label htmlFor="ads-booking">Booking action ID</label><input id="ads-booking" inputMode="numeric" pattern="[0-9]{1,20}" maxLength={20} value={draft.booking_action_id} onChange={e => setDraft({ ...draft, booking_action_id: e.target.value })} /></div>
    <label className="site-toggle"><input type="checkbox" checked={draft.allow_inferred_calls} onChange={e => setDraft({ ...draft, allow_inferred_calls: e.target.checked })} />Allow preparing calls with inferred website attribution</label>
    <p className="muted">Use separate conversion actions for qualification and bookings. Choose your primary optimization action in Google Ads to avoid counting both stages as separate sales.</p>
    <div><button className="primary" disabled={busy}>Save destination</button></div>
  </form></details>;
}
function ConsentForm({ value, busy, save }: { value: Consent; busy: boolean; save: (c: Consent) => Promise<void> }) {
  const [draft, setDraft] = useState(value);
  const [time, setTime] = useState(value.effective_at?.slice(0, 19) ?? "");
  function submit(e: FormEvent) { e.preventDefault(); void save({ ...draft, effective_at: time ? new Date(`${time}Z`).toISOString() : null }); }
  return <details className="conversion-consent"><summary>Record advertising consent evidence</summary><p className="muted">Record a decision already made by this person, with its original time and evidence reference. This form does not obtain consent. General website tracking consent is insufficient. A withdrawal should be recorded here as denied. Avoid personal details in the reference.</p><form className="site-form" onSubmit={submit}>
    {([ ["ad_user_data", "Advertising user-data consent"], ["ad_personalization", "Advertising personalization consent"] ] as const).map(([field, title]) => <div className="form-field" key={field}><label htmlFor={`consent-${field}`}>{title}</label><select id={`consent-${field}`} value={draft[field]} onChange={e => setDraft({ ...draft, [field]: e.target.value as Decision })}><option value="unknown">Unknown</option><option value="granted">Granted</option><option value="denied">Denied</option></select></div>)}
    <div className="form-field"><label htmlFor="consent-time">Decision effective at (UTC)</label><input id="consent-time" type="datetime-local" step="1" value={time} onChange={e => setTime(e.target.value)} /></div>
    <div className="form-field"><label htmlFor="consent-reference">Consent evidence reference</label><input id="consent-reference" maxLength={300} value={draft.reference} onChange={e => setDraft({ ...draft, reference: e.target.value })} placeholder="Consent manager receipt or policy record ID" /></div>
    <div><button className="secondary" disabled={busy}>Save consent evidence</button></div>
  </form></details>;
}
