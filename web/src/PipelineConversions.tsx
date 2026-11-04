import { useEffect, useState, type FormEvent } from "react";
import { type Client, money, label } from "./api";
import type { Discovery, GoogleAction } from "./GoogleConnection";

type Action = { key: string; name: string; trigger: "events" | "new_lead"; default_value_minor: number | null; counting: "once_per_click" | "every"; google_action_id: string | null; enabled: boolean; version: number };
type Policy = { enabled: boolean; auto_send: boolean; consent_policy: "per_lead" | "not_required_outside_eea"; attested_by: string | null; attested_at: string | null; version: number };
type Event = { id: string; action: string; external_id: string; lead_id: string | null; matched_by: string; value_minor: number | null; occurred_at: string; revision: number; status: string; delivery: { google: string; reason: string | null; request_id: string | null } };
type Setup = { ready: boolean; summary: string; next_steps: { actor: string; step: string }[] };
type Overview = { actions: Action[]; policy: Policy; events: Event[]; setup: Setup };
const blank: Action = { key: "", name: "", trigger: "events", default_value_minor: null, counting: "every", google_action_id: "", enabled: true, version: 0 };
const ATTESTATION = "I confirm this workspace does not advertise to people in the EEA, the UK or Switzerland, and I am authorized to make this statement for the business.";
// Whole cents from a dollar string without floating-point arithmetic.
function cents(text: string): number | null {
  const t = text.trim();
  if (!t) return null;
  if (!/^\d+(\.\d{1,2})?$/.test(t)) throw new Error("Enter an amount like 20 or 20.00");
  const [whole, part = ""] = t.split(".");
  return Number(whole) * 100 + Number(part.padEnd(2, "0"));
}
const dollars = (minor: number | null) => minor === null ? "" : `${Math.floor(minor / 100)}.${String(minor % 100).padStart(2, "0")}`;

// `revision` is the shared delivery settings version; a change elsewhere on the page refetches.
// `settingsVersion` is the saved Google destination; saving it refreshes the checklist.
export function PipelineConversions({ api, revision, settingsVersion, discovery, onChange }: { api: Client; revision: number; settingsVersion: number; discovery: Discovery | null; onChange: () => Promise<void> }) {
  const [data, setData] = useState<Overview | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState<string | null>(null);
  async function refresh() { setData(await api<Overview>("/conversions/pipeline")); }
  useEffect(() => { let active = true; api<Overview>("/conversions/pipeline").then(d => { if (active) setData(d); }).catch(e => { if (active) setError(e.message); }); return () => { active = false; }; }, [api, revision, settingsVersion]);
  async function mutate(path: string, body: unknown, message: string, method = "PUT") {
    setBusy(true); setError(""); setNotice("");
    try { await api(path, body, method); await refresh(); await onChange(); setNotice(message); setEditing(null); return true; }
    catch (e) { setError((e as Error).message); await refresh().catch(() => {}); return false; }
    finally { setBusy(false); }
  }
  if (!data) return <section className="card site-panel" aria-label="Conversion actions"><p>{error || "Loading automatic conversions…"}</p></section>;
  const { policy, setup } = data;
  const googleActions = discovery?.actions.actions ?? null;
  const googleName = (id: string | null) => googleActions?.find(g => g.id === id)?.name;
  const status = <>{error && <p className="error" role="alert">{error}</p>}{notice && <p className="notice" role="status">{notice}</p>}</>;
  return <>
    <section className="card site-panel" aria-label="Conversion actions">
      <div><span className="eyebrow">STEP 2 · YOUR ACTIONS</span><h2>Conversion actions</h2></div>
      {status}
      <p className="muted">Each action is one step of your pipeline, like <code>lead</code>, <code>quote_sent</code> or <code>order</code>. Map it to an existing Google Ads import conversion action, or create a new VisiConvert action in Google for it. <strong>Once per click</strong> counts a single conversion per ad click (like a lead or first quote); <strong>every event</strong> counts each event (like each order). An action’s key, trigger and counting cannot change after it is created. Check the saved connection in step 1 after mapping a new action.</p>
      {!data.actions.length ? <p>No actions yet. Add one for each step you want Google Ads to count.</p> : <div className="conversion-jobs">{data.actions.map(a => <article className="conversion-job" key={a.key} aria-label={`Action ${a.key}`}>
        <strong>{a.name} · <code>{a.key}</code></strong><span className="badge">{a.enabled ? "Enabled" : "Disabled"}</span>
        <small>{a.trigger === "new_lead" ? "Recorded automatically for each new attributed lead" : "Posted by your system"} · {a.counting === "once_per_click" ? "Once per click" : "Every event"} · {a.default_value_minor === null ? "No default value" : `Default ${money(a.default_value_minor)}`} · {a.google_action_id ? `Google action ${googleName(a.google_action_id) ? `${googleName(a.google_action_id)} · ` : ""}${a.google_action_id}` : "Not mapped to Google"}</small>
        {editing === a.key ? <ActionForm value={a} googleActions={googleActions} busy={busy} save={v => mutate(`/conversions/actions/${a.key}`, v, "Action saved.")} cancel={() => setEditing(null)} /> : <div className="site-actions"><button className="secondary" disabled={busy} onClick={() => setEditing(a.key)}>Edit action</button>
          {!a.google_action_id && discovery && <button className="secondary" disabled={busy || discovery.actions.account.conversion_owner !== discovery.customer} onClick={() => void mutate(`/conversion-actions/${a.key}/google-action`, { version: a.version, login: discovery.login }, "Google conversion action created or recovered and mapped. Check the saved connection in step 1.", "POST")}>Create in Google Ads</button>}</div>}
      </article>)}</div>}
      {editing === "" ? <ActionForm value={blank} googleActions={googleActions} busy={busy} save={v => mutate(`/conversions/actions/${v.key}`, v, "Action created.")} cancel={() => setEditing(null)} /> : <div><button className="primary" disabled={busy} onClick={() => setEditing("")}>Add action</button></div>}
      {!discovery && <p className="muted">Choose and save the advertising account in step 1 to pick Google actions from a list. You can also enter a numeric action ID.</p>}
      {discovery && <p className="muted"><strong>Create in Google Ads</strong> makes a secondary (reporting only) import action named after the action key, with a 90-day click window. A retry finds the action an earlier attempt created. Prefer an existing action if you already have one.</p>}
    </section>
    <section className="card site-panel" aria-label="Advertising consent">
      <div><span className="eyebrow">STEP 3 · CONSENT</span><h2>Advertising consent</h2></div>
      <ConsentPolicy key={`${policy.version}`} policy={policy} busy={busy} save={(consent_policy, attest) => mutate("/conversions/pipeline/policy", { auto_send: policy.auto_send, consent_policy, attest, version: policy.version }, "Consent policy saved.")} />
    </section>
    <section className="card site-panel" aria-label="Automatic sending">
      <div className="section-heading"><div><span className="eyebrow">STEP 4 · SENDING</span><h2>{policy.enabled && policy.auto_send ? "Automatic sending on" : "Automatic sending off"}</h2></div></div>
      <p className="muted">Your CRM or booking system posts events to <code>/api/v1/conversion-events</code> with an API key or agent connection that has the <code>conversions:write</code> permission. VisiConvert finds the lead and its Google click, checks the click window and consent, and sends the conversion. Events are always recorded; they are sent only while Google delivery and automatic sending are both on. Events recorded while sending is off are never sent later. Only the workspace owner can turn these on; API keys and agents cannot.</p>
      <p className="muted">Turn off any other uploader for the same Google conversion actions the same day you turn this on, or each conversion counts twice.</p>
      <div className="site-actions">
        <button className="secondary" disabled={busy} onClick={() => void mutate("/conversions/delivery/settings", { enabled: !policy.enabled, version: policy.version }, policy.enabled ? "Google delivery paused. Queued sends wait until you resume." : "Google delivery turned on.")}>{policy.enabled ? "Pause Google delivery" : "Turn on Google delivery"}</button>
        <button className="primary" disabled={busy} onClick={() => void mutate("/conversions/pipeline/policy", { auto_send: !policy.auto_send, consent_policy: policy.consent_policy, attest: policy.consent_policy === "not_required_outside_eea", version: policy.version }, policy.auto_send ? "Automatic sending turned off. Events are still recorded." : "Automatic sending turned on. New eligible events are sent to Google.")}>{policy.auto_send ? "Turn off automatic sending" : "Turn on automatic sending"}</button>
      </div>
      <p>Google delivery: <strong>{policy.enabled ? "on" : "paused"}</strong> (shared with manual review) · Automatic sending: <strong>{policy.auto_send ? "on" : "off"}</strong></p>
      <div className={setup.ready ? "notice" : "muted"} aria-label="Setup checklist"><strong>{setup.ready ? "Ready" : "Still to do"}</strong>{!setup.ready && <ul>{setup.next_steps.map(s => <li key={s.step}>{s.step}</li>)}</ul>}{setup.ready && <p>{setup.summary}</p>}</div>
    </section>
    <section className="card site-panel" aria-label="Recent pipeline events">
      <div className="section-heading"><div><span className="eyebrow">STEP 5 · RESULTS</span><h2>Recent pipeline events</h2></div><button className="secondary" disabled={busy} onClick={() => { setError(""); void refresh().catch(e => setError(e.message)); }}>Refresh events</button></div>
      <p className="muted">Latest 100 events. Blocked events are kept with the reason they were not sent. Delivered means Google processed the request; it does not prove attribution.</p>
      {!data.events.length ? <p>No events recorded yet.</p> : <div className="conversion-jobs">{data.events.map(e => <article className="conversion-job" key={e.id} aria-label={`Event ${e.action} ${e.external_id}`}>
        <strong>{e.action} · {e.external_id}</strong><span className="badge">{label(e.delivery.google)}</span>
        <small>Revision {e.revision} · {e.value_minor === null ? "No value" : money(e.value_minor)} · {new Date(e.occurred_at).toLocaleString()} · matched by {label(e.matched_by)}</small>
        {e.delivery.reason && <p>{e.delivery.reason}</p>}
        {e.delivery.request_id && <small>Google request ID: <code>{e.delivery.request_id}</code></small>}
      </article>)}</div>}
    </section>
  </>;
}
function ConsentPolicy({ policy, busy, save }: { policy: Policy; busy: boolean; save: (p: Policy["consent_policy"], attest: boolean) => Promise<boolean> }) {
  const [choice, setChoice] = useState(policy.consent_policy);
  const [attest, setAttest] = useState(false);
  const changed = choice !== policy.consent_policy;
  return <fieldset className="site-form"><legend>Consent policy</legend>
    <label className="site-toggle"><input type="radio" name="consent-policy" checked={choice === "per_lead"} onChange={() => setChoice("per_lead")} />Send only leads with recorded advertising consent</label>
    <label className="site-toggle"><input type="radio" name="consent-policy" checked={choice === "not_required_outside_eea"} onChange={() => setChoice("not_required_outside_eea")} />Not required: this workspace does not advertise in the EEA, UK or Switzerland</label>
    {policy.consent_policy === "not_required_outside_eea" && policy.attested_at && <p className="muted">Attested by {policy.attested_by} on {new Date(policy.attested_at).toLocaleString()}. Conversions are sent without a consent signal.</p>}
    {changed && choice === "not_required_outside_eea" && <label className="site-toggle"><input type="checkbox" checked={attest} onChange={e => setAttest(e.target.checked)} />{ATTESTATION}</label>}
    {changed && <div><button className="secondary" disabled={busy || (choice === "not_required_outside_eea" && !attest)} onClick={e => { e.preventDefault(); void save(choice, choice === "not_required_outside_eea"); }}>Save consent policy</button></div>}
  </fieldset>;
}
function ActionForm({ value, googleActions, busy, save, cancel }: { value: Action; googleActions: GoogleAction[] | null; busy: boolean; save: (a: Action) => Promise<boolean>; cancel: () => void }) {
  const [draft, setDraft] = useState(value);
  const [amount, setAmount] = useState(dollars(value.default_value_minor));
  const [error, setError] = useState("");
  const fixed = value.version > 0;
  const id = value.key || "new";
  function submit(e: FormEvent) {
    e.preventDefault(); setError("");
    // Send only the editable fields; saved rows also carry timestamps the API rejects.
    const { key, name, trigger, counting, enabled, version } = draft;
    try { void save({ key, name, trigger, counting, enabled, version, default_value_minor: cents(amount), google_action_id: draft.google_action_id || null }); }
    catch (err) { setError((err as Error).message); }
  }
  return <form className="site-form" onSubmit={submit} aria-label={fixed ? `Edit action ${value.key}` : "New action"}>
    {error && <p className="error" role="alert">{error}</p>}
    <div className="form-field"><label htmlFor={`action-key-${id}`}>Action key</label><input id={`action-key-${id}`} required disabled={fixed} pattern="[a-z][a-z0-9_]{0,39}" maxLength={40} value={draft.key} onChange={e => setDraft({ ...draft, key: e.target.value })} placeholder="quote_sent" /></div>
    <div className="form-field"><label htmlFor={`action-name-${id}`}>Action name</label><input id={`action-name-${id}`} required maxLength={100} value={draft.name} onChange={e => setDraft({ ...draft, name: e.target.value })} /></div>
    <div className="form-field"><label htmlFor={`action-trigger-${id}`}>Trigger</label><select id={`action-trigger-${id}`} disabled={fixed} value={draft.trigger} onChange={e => setDraft({ ...draft, trigger: e.target.value as Action["trigger"] })}><option value="events">Posted by your system</option><option value="new_lead">Each new attributed lead</option></select></div>
    <div className="form-field"><label htmlFor={`action-counting-${id}`}>Counting</label><select id={`action-counting-${id}`} disabled={fixed} value={draft.counting} onChange={e => setDraft({ ...draft, counting: e.target.value as Action["counting"] })}><option value="once_per_click">Once per click</option><option value="every">Every event</option></select></div>
    <div className="form-field"><label htmlFor={`action-value-${id}`}>Default value (USD, optional)</label><input id={`action-value-${id}`} inputMode="decimal" value={amount} onChange={e => setAmount(e.target.value)} placeholder="20.00" /></div>
    {googleActions ? <div className="form-field"><label htmlFor={`action-google-${id}`}>Google conversion action</label><select id={`action-google-${id}`} value={draft.google_action_id ?? ""} onChange={e => setDraft({ ...draft, google_action_id: e.target.value })}><option value="">Not mapped</option>{draft.google_action_id && !googleActions.some(g => g.id === draft.google_action_id) && <option value={draft.google_action_id}>Saved action {draft.google_action_id} · not in this list</option>}{googleActions.map(g => <option key={g.id} value={g.id} disabled={!g.eligible}>{g.name} · {g.id}{g.eligible ? (g.primary ? " · primary" : " · secondary") : ` · ${g.kind} / ${g.status}`}</option>)}</select></div>
    : <div className="form-field"><label htmlFor={`action-google-${id}`}>Google conversion action ID</label><input id={`action-google-${id}`} inputMode="numeric" pattern="[0-9]{1,20}" maxLength={20} value={draft.google_action_id ?? ""} onChange={e => setDraft({ ...draft, google_action_id: e.target.value })} /></div>}
    <label className="site-toggle"><input type="checkbox" checked={draft.enabled} onChange={e => setDraft({ ...draft, enabled: e.target.checked })} />Enabled</label>
    <div className="site-actions"><button className="primary" disabled={busy}>{fixed ? "Save action" : "Create action"}</button><button type="button" className="secondary" disabled={busy} onClick={cancel}>Cancel</button></div>
  </form>;
}
