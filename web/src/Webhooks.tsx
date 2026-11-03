import { useEffect, useState, type FormEvent } from "react";
import { type Client } from "./api";

type Endpoint = {
  id: string; url: string; description: string; event_types: string[]; include_contact: boolean;
  state: "enabled" | "paused" | "disabled"; disabled_reason: string | null; failing_since: string | null;
  secret_rotating_until: string | null; version: number; created_at: string;
};
type Data = { endpoints: Endpoint[]; event_types: string[]; history: { id: number; endpoint_id: string; kind: string; actor: string; created_at: string }[] };
type Delivery = { id: string; event_id: string; type: string; state: string; attempts: number; last_status: number | null; last_error: string | null; last_attempt_at: string | null; next_attempt_at: string | null; created_at: string };
type Page = { data: Delivery[]; next_cursor: string | null };

export const eventLabels: Record<string, string> = {
  "interaction.created": "New call or form lead",
  "call.completed": "Call finished",
  "inquiry.reviewed": "Inquiry qualified, unqualified, or marked spam",
  "booking.recorded": "Booking recorded",
  "booking.revised": "Booking revised",
  "booking.canceled": "Booking canceled",
  "conversion.delivered": "Google Ads accepted a conversion",
  "conversion.failed": "Google Ads conversion failed or needs review",
};
const defaults = ["interaction.created", "call.completed", "inquiry.reviewed", "booking.recorded", "booking.revised", "booking.canceled"];
const stateLabel = { enabled: "Active", paused: "Paused", disabled: "Disabled" };
const when = (value: string | null) => value ? new Date(value).toLocaleString() : "—";

function Form({ initial, onSubmit, submit, busy, onCancel }: { initial: Pick<Endpoint, "url" | "description" | "event_types" | "include_contact">; onSubmit: (v: typeof initial) => void; submit: string; busy: boolean; onCancel?: () => void }) {
  const [url, setUrl] = useState(initial.url);
  const [description, setDescription] = useState(initial.description);
  const [events, setEvents] = useState(initial.event_types);
  const [contact, setContact] = useState(initial.include_contact);
  const id = submit.replace(/\W+/g, "-").toLowerCase();
  function send(e: FormEvent) { e.preventDefault(); onSubmit({ url, description, event_types: events, include_contact: contact }); }
  return <form className="site-form" onSubmit={send}>
    <div className="form-field"><label htmlFor={`${id}-url`}>Endpoint URL</label><input id={`${id}-url`} type="url" required maxLength={2000} value={url} onChange={e => setUrl(e.target.value)} placeholder="https://crm.example.com/hooks/visiconvert" /></div>
    <div className="form-field"><label htmlFor={`${id}-description`}>Description (optional)</label><input id={`${id}-description`} maxLength={200} value={description} onChange={e => setDescription(e.target.value)} placeholder="CRM sync" /></div>
    <fieldset className="webhook-events"><legend>Events to send</legend>
      {Object.entries(eventLabels).map(([type, title]) => <label className="site-toggle" key={type}><input type="checkbox" checked={events.includes(type)} onChange={e => setEvents(e.target.checked ? [...events, type] : events.filter(t => t !== type))} />{title} <code>{type}</code></label>)}
    </fieldset>
    <label className="site-toggle"><input type="checkbox" checked={contact} onChange={e => setContact(e.target.checked)} />Include contact details: name, phone or email, notes, and form fields</label>
    <div className="site-actions"><button className="primary" disabled={busy || !events.length}>{submit}</button>{onCancel && <button type="button" className="secondary" disabled={busy} onClick={onCancel}>Cancel</button>}</div>
  </form>;
}

function Deliveries({ api, endpoint, tick, onError }: { api: Client; endpoint: Endpoint; tick: number; onError: (m: string) => void }) {
  const [rows, setRows] = useState<Delivery[] | null>(null);
  const [cursor, setCursor] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  async function load(after?: string | null) {
    const page = await api<Page>(`/webhooks/${endpoint.id}/deliveries?limit=20${after ? `&cursor=${encodeURIComponent(after)}` : ""}`);
    setRows(old => after && old ? [...old, ...page.data] : page.data); setCursor(page.next_cursor);
  }
  useEffect(() => { void load().catch(e => onError(e.message)); }, [endpoint.id, endpoint.version, tick])
  // While something is about to be sent, check again shortly so results appear without a manual refresh.
  const waiting = rows?.some(d => d.state === "sending" || ((d.state === "pending" || d.state === "retry") && (!d.next_attempt_at || new Date(d.next_attempt_at).getTime() - Date.now() < 60000)));
  useEffect(() => { if (!waiting) return; const t = setTimeout(() => void load().catch(() => {}), 2000); return () => clearTimeout(t); }, [rows, waiting])
  async function redeliver(id: string) {
    setBusy(true);
    try { await api(`/webhook-deliveries/${id}/redeliver`, {}); await load(); } catch (e) { onError((e as Error).message); } finally { setBusy(false); }
  }
  return <div className="webhook-deliveries">
    <div className="section-heading"><h3>Recent deliveries</h3><button className="secondary" disabled={busy} onClick={() => void load().catch(e => onError(e.message))}>Refresh deliveries</button></div>
    {!rows ? <p>Loading deliveries…</p> : !rows.length ? <p className="muted">No deliveries yet. Send a test to check your receiver.</p> :
      <ul className="webhook-delivery-list">{rows.map(d => <li key={d.id} aria-label={`Delivery ${d.type} ${d.state}`}>
        <strong>{d.type}</strong><span className={`badge delivery-${d.state}`}>{d.state}</span>
        <small>{d.attempts} attempt{d.attempts === 1 ? "" : "s"}{d.last_status ? ` · HTTP ${d.last_status}` : ""} · {when(d.last_attempt_at ?? d.created_at)}{d.next_attempt_at ? ` · next try ${when(d.next_attempt_at)}` : ""}</small>
        {d.last_error && <small className="muted">{d.last_error}</small>}
        {["succeeded", "failed", "canceled"].includes(d.state) && endpoint.state === "enabled" && <button className="secondary" disabled={busy} onClick={() => void redeliver(d.id)}>Redeliver</button>}
      </li>)}</ul>}
    {cursor && <button className="secondary" disabled={busy} onClick={() => void load(cursor).catch(e => onError(e.message))}>Load older deliveries</button>}
  </div>;
}

export function Webhooks({ api }: { api: Client }) {
  const [data, setData] = useState<Data | null>(null);
  const [secret, setSecret] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState<string | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [confirmation, setConfirmation] = useState<{ endpoint: Endpoint; action: "rotate" | "delete" } | null>(null);
  const [formKey, setFormKey] = useState(0);
  const [tick, setTick] = useState(0);
  async function refresh() { setData(await api<Data>("/webhooks")); }
  useEffect(() => { let active = true; api<Data>("/webhooks").then(d => { if (active) setData(d); }).catch(e => { if (active) setError(e.message); }); return () => { active = false; }; }, [api]);
  async function run<T>(action: () => Promise<T>, message: (result: T) => string) {
    setBusy(true); setError(""); setNotice("");
    try { const result = await action(); await refresh(); setNotice(message(result)); return result; }
    catch (e) { setError((e as Error).message); }
    finally { setBusy(false); }
  }
  const body = (e: Endpoint, changes: Partial<Endpoint> = {}) => ({ url: e.url, description: e.description, event_types: e.event_types, include_contact: e.include_contact, version: e.version, ...changes });
  const create = (v: Pick<Endpoint, "url" | "description" | "event_types" | "include_contact">) => void run(async () => {
    const created = await api<Endpoint & { secret: string }>("/webhooks", v);
    setSecret(created.secret); setFormKey(k => k + 1); setOpen(created.id); return created;
  }, () => "Endpoint created. Save its signing secret now; it is shown only once.");
  const rotate = (e: Endpoint) => void run(async () => {
    const rotated = await api<Endpoint & { secret: string }>(`/webhooks/${e.id}/rotate-secret`, { version: e.version });
    setSecret(rotated.secret); setConfirmation(null); return rotated;
  }, () => "New secret issued. Requests carry signatures from both secrets for 24 hours.");
  return <div className="conversion-layout">
    {error && <p className="error" role="alert">{error}</p>}
    {notice && <p className="notice" role="status">{notice}</p>}
    {secret && <section className="card site-panel"><h2>Save your signing secret</h2><p>Your receiver uses this secret to check that requests came from VisiConvert. It cannot be retrieved again. Leaving this page clears it from the screen.</p><textarea aria-label="Webhook signing secret" className="site-code" readOnly value={secret} rows={2} /><button className="secondary" onClick={() => { setSecret(""); setNotice(""); }}>I saved the secret</button></section>}
    <section className="card site-panel"><h2>Add an endpoint</h2>
      <p className="muted">VisiConvert sends a signed HTTPS request to your CRM, Zapier, Make, n8n, or your own service when these events happen. Up to 10 endpoints. Requests go only to public HTTPS addresses on port 443.</p>
      <Form key={formKey} initial={{ url: "", description: "", event_types: defaults, include_contact: true }} onSubmit={create} submit="Add endpoint" busy={busy || !!secret} />
    </section>
    <section className="card site-panel"><div className="section-heading"><h2>Endpoints</h2><button className="secondary" disabled={busy} onClick={() => void refresh().catch(e => setError(e.message))}>Refresh endpoints</button></div>
      {!data ? <p>Loading endpoints…</p> : !data.endpoints.length ? <p>No webhook endpoints yet.</p> : <div className="conversion-jobs">{data.endpoints.map(e => <article className="conversion-job" key={e.id} aria-label={`Webhook ${e.description || e.url}`}>
        <strong>{e.description || e.url}</strong>
        {e.description && <code>{e.url}</code>}
        <span className={`badge webhook-${e.state}`}>{stateLabel[e.state]}</span>
        {e.state === "disabled" && <p className="error">{e.disabled_reason || "Disabled"}. Fix the receiver, then re-enable. Queued deliveries were canceled; redeliver any you need.</p>}
        {e.state === "enabled" && e.failing_since && <p className="notice">Deliveries have been failing since {when(e.failing_since)}. The endpoint is disabled after 5 days of failures.</p>}
        <small>Events: {e.event_types.join(", ")}</small>
        <small>{e.include_contact ? "Includes contact details" : "Contact details left out"}{e.secret_rotating_until ? ` · Previous secret also signs until ${when(e.secret_rotating_until)}` : ""}</small>
        {editing === e.id ? <Form initial={e} submit="Save endpoint" busy={busy} onCancel={() => setEditing(null)} onSubmit={v => void run(async () => { await api(`/webhooks/${e.id}`, body(e, v), "PATCH"); setEditing(null); }, () => "Endpoint saved.")} /> :
          <div className="site-actions">
            <button className="secondary" disabled={busy || e.state !== "enabled"} onClick={() => void run(async () => { await api(`/webhooks/${e.id}/test`, {}); setOpen(e.id); setTick(t => t + 1); }, () => "Test event queued.")}>Send test</button>
            {e.state === "enabled" ? <button className="secondary" disabled={busy} onClick={() => void run(() => api(`/webhooks/${e.id}`, body(e, { state: "paused" }), "PATCH"), () => "Endpoint paused. New events wait until you resume it.")}>Pause</button>
              : <button className="secondary" disabled={busy} onClick={() => void run(() => api(`/webhooks/${e.id}`, body(e, { state: "enabled" }), "PATCH"), () => e.state === "paused" ? "Endpoint resumed." : "Endpoint re-enabled.")}>{e.state === "paused" ? "Resume" : "Re-enable"}</button>}
            <button className="secondary" disabled={busy} onClick={() => setEditing(e.id)}>Edit</button>
            <button className="secondary" disabled={busy || !!secret} onClick={() => setConfirmation({ endpoint: e, action: "rotate" })}>Rotate secret</button>
            <button className="secondary" disabled={busy} onClick={() => setConfirmation({ endpoint: e, action: "delete" })}>Delete</button>
            <button className="secondary" aria-expanded={open === e.id} onClick={() => setOpen(open === e.id ? null : e.id)}>{open === e.id ? "Hide deliveries" : "Show deliveries"}</button>
          </div>}
        {confirmation?.endpoint.id === e.id && <div className="notice"><p>{confirmation.action === "rotate" ? "Issue a new signing secret? The current secret keeps signing for 24 hours so you can update your receiver." : "Delete this endpoint? Queued deliveries are discarded and nothing more is sent."}</p><div className="site-actions"><button className="primary" disabled={busy} onClick={() => confirmation.action === "rotate" ? rotate(e) : void run(async () => { await api(`/webhooks/${e.id}`, undefined, "DELETE"); setConfirmation(null); }, () => "Endpoint deleted.")}>{confirmation.action === "rotate" ? "Confirm rotation" : "Confirm delete"}</button><button className="secondary" disabled={busy} onClick={() => setConfirmation(null)}>Cancel</button></div></div>}
        {open === e.id && <Deliveries api={api} endpoint={e} tick={tick} onError={setError} />}
      </article>)}</div>}
    </section>
    <section className="card site-panel"><h2>Verify requests</h2>
      <p>Requests follow the Standard Webhooks format. Check the <code>webhook-signature</code> header with your secret and the raw request body before trusting a request, and ignore duplicates by <code>webhook-id</code>. Failed deliveries are retried for about 28 hours.</p>
      <pre className="site-code">{`import { Webhook } from "standardwebhooks";\nconst wh = new Webhook(process.env.VISICONVERT_WEBHOOK_SECRET.replace("whsec_", ""));\nconst event = wh.verify(rawBody, request.headers);`}</pre>
      <a href="https://docs.visiconvert.com/guides/webhooks/" target="_blank" rel="noreferrer">Webhooks guide</a>
    </section>
    <details className="card site-panel"><summary>Endpoint history (latest 100)</summary><ul className="inquiry-history">{data?.history.map(h => <li key={h.id}>{h.kind.replace("_", " ")} · {h.actor} · {new Date(h.created_at).toLocaleString()}</li>)}</ul></details>
  </div>;
}
