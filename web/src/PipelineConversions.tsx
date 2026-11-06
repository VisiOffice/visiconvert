import { useEffect, useState, type FormEvent } from "react";
import { type Client, money, label } from "./api";
import type { Discovery, GoogleAction } from "./GoogleConnection";
import type { Panel, Tab } from "./Conversions";

type Action = { key: string; name: string; trigger: "events" | "new_lead"; default_value_minor: number | null; counting: "once_per_click" | "every"; google_action_id: string | null; enabled: boolean; version: number; microsoft_goal_name?: string | null };
type Policy = { enabled: boolean; auto_send: boolean; consent_policy: "per_lead" | "not_required_outside_eea"; attested_by: string | null; attested_at: string | null; version: number };
type Event = { id: string; action: string; external_id: string; lead_id: string | null; matched_by: string; value_minor: number | null; occurred_at: string; revision: number; status: string; delivery: { google: string; reason: string | null; request_id: string | null; microsoft?: { status: string; reason: string | null; tracking_id: string | null } | null } };
type Step = { actor: string; step: string; advisory?: boolean };
type MicrosoftSetup = { ready: boolean; summary: string; next_steps: Step[]; configured?: boolean; connected?: boolean; account_name?: string | null; account_id?: string | null; mapped_actions?: number; enabled_actions?: number; events_sent?: number };
export type Setup = { ready: boolean; summary: string; next_steps: Step[]; google_connected?: boolean; customer_id?: string | null; mapped_actions?: number; enabled_actions?: number; microsoft?: MicrosoftSetup };
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
export function PipelineConversions({ api, revision, settingsVersion, refreshKey = 0, discovery, onChange, onSetup, tab, panel, open }: { api: Client; revision: number; settingsVersion: number; refreshKey?: number; discovery: Discovery | null; onChange: () => Promise<void>; onSetup: (s: Setup) => void; tab: Tab; panel: Panel; open: (t: Tab) => void }) {
  const [data, setData] = useState<Overview | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState<string | null>(null);
  async function refresh() { setData(await api<Overview>("/conversions/pipeline")); }
  useEffect(() => { let active = true; api<Overview>("/conversions/pipeline").then(d => { if (active) setData(d); }).catch(e => { if (active) setError(e.message); }); return () => { active = false; }; }, [api, revision, settingsVersion, refreshKey]);
  async function mutate(path: string, body: unknown, message: string, method = "PUT") {
    setBusy(true); setError(""); setNotice("");
    try { await api(path, body, method); await refresh(); await onChange(); setNotice(message); setEditing(null); return true; }
    catch (e) { setError((e as Error).message); await refresh().catch(() => {}); return false; }
    finally { setBusy(false); }
  }
  useEffect(() => { if (data) onSetup(data.setup); }, [data]);
  // A message belongs to the tab where it happened.
  useEffect(() => { setNotice(""); setError(""); }, [tab]);
  if (!data) return <div {...panel(tab === "actions" || tab === "events" ? tab : "overview")} hidden={!["overview", "actions", "events"].includes(tab)}><section className="card site-panel"><p>{error || "Loading automatic conversions…"}</p></section></div>;
  const { policy, setup } = data;
  const googleActions = discovery?.actions.actions ?? null;
  const googleName = (id: string | null) => googleActions?.find(g => g.id === id)?.name;
  const status = <>{error && <p className="error" role="alert">{error}</p>}{notice && <p className="notice" role="status">{notice}</p>}</>;
  return <>
    <div {...panel("overview")}>
      {tab === "overview" && status}
      <section className="destination-grid" aria-label="Destinations">
        <Destination name="Google Ads" open={() => open("google")} state={setup.ready ? "ok" : setup.google_connected ? "todo" : "off"} label={setup.ready ? "Sending" : setup.google_connected ? "Needs setup" : "Not connected"}
          detail={setup.enabled_actions ? `${setup.mapped_actions ?? 0} of ${setup.enabled_actions} actions mapped${setup.customer_id ? ` · account ${setup.customer_id.replace(/^(\d{3})(\d{3})(\d{4})$/, "$1-$2-$3")}` : ""}` : "No actions yet"}
          summary={setup.summary} steps={setup.ready ? [] : setup.next_steps} checklist="Setup checklist" />
        {setup.microsoft && <Destination name="Microsoft Advertising" open={() => open("microsoft")} state={!setup.microsoft.configured ? "off" : setup.microsoft.ready ? "ok" : setup.microsoft.connected ? "todo" : "off"} label={!setup.microsoft.configured ? "Unavailable" : setup.microsoft.ready ? "Sending" : setup.microsoft.connected ? "Needs setup" : "Not connected"}
          detail={setup.microsoft.connected ? `${setup.microsoft.mapped_actions ?? 0} of ${setup.microsoft.enabled_actions ?? 0} actions mapped${setup.microsoft.account_name ? ` · ${setup.microsoft.account_name}` : ""} · ${setup.microsoft.events_sent ?? 0} sent` : "Optional second destination for the same actions"}
          summary={setup.microsoft.summary} steps={setup.microsoft.ready ? setup.microsoft.next_steps.filter(s => s.advisory) : setup.microsoft.next_steps} checklist="Microsoft setup summary" />}
      </section>
    <section className="card site-panel" aria-label="Automatic sending">
      <div className="section-heading"><div><span className="eyebrow">SENDING</span><h2>{policy.enabled && policy.auto_send ? "Automatic sending on" : policy.auto_send ? "Automatic sending paused: delivery is off" : "Automatic sending off"}</h2></div></div>
      <p className="muted">Your CRM or booking system posts events to <code>/api/v1/conversion-events</code> (permission <code>conversions:write</code>). VisiConvert finds the lead and its ad click, checks the click window and consent, and sends the conversion to every mapped destination. Events are always recorded, but only sent while delivery and automatic sending are both on; events recorded while off are never sent later. Only the workspace owner can turn these on.</p>
      <p className="muted">Turn off any other uploader for the same conversion actions or goals the same day, or each conversion counts twice.</p>
      <div className="site-actions">
        <button className="secondary" disabled={busy} onClick={() => void mutate("/conversions/delivery/settings", { enabled: !policy.enabled, version: policy.version }, policy.enabled ? "Google delivery paused. Queued sends wait until you resume." : "Google delivery turned on.")}>{policy.enabled ? "Pause Google delivery" : "Turn on Google delivery"}</button>
        <button className="primary" disabled={busy} onClick={() => void mutate("/conversions/pipeline/policy", { auto_send: !policy.auto_send, consent_policy: policy.consent_policy, attest: policy.consent_policy === "not_required_outside_eea", version: policy.version }, policy.auto_send ? "Automatic sending turned off. Events are still recorded." : "Automatic sending turned on. New eligible events are sent to Google.")}>{policy.auto_send ? "Turn off automatic sending" : "Turn on automatic sending"}</button>
      </div>
      <p className="sending-state"><span className={`state-pill ${policy.enabled ? "on" : ""}`}>Delivery {policy.enabled ? "on" : "paused"}</span><span className={`state-pill ${policy.auto_send ? "on" : ""}`}>Automatic sending {policy.auto_send ? "on" : "off"}</span><small>Delivery is shared by Google, Microsoft and manual review.</small></p>
    </section>
    <section className="card site-panel" aria-label="Advertising consent">
      <div><span className="eyebrow">CONSENT</span><h2>Advertising consent</h2></div>
      <ConsentPolicy key={`${policy.version}`} policy={policy} busy={busy} save={(consent_policy, attest) => mutate("/conversions/pipeline/policy", { auto_send: policy.auto_send, consent_policy, attest, version: policy.version }, "Consent policy saved.")} />
    </section>
    </div>
    <div {...panel("actions")}>
      {tab === "actions" && status}
    <section className="card site-panel" aria-label="Conversion actions">
      <div className="section-heading"><div><span className="eyebrow">PIPELINE STEPS</span><h2>Conversion actions</h2></div><span>{data.actions.filter(a => a.enabled).length} enabled</span></div>
      <p className="muted">Each action is one step of your pipeline, like <code>lead</code>, <code>quote_sent</code> or <code>order</code>. <strong>Once per click</strong> counts one conversion per ad click; <strong>every event</strong> counts each one (like each order). Key, trigger and counting are fixed once created. Map goals for Microsoft on the Microsoft Ads tab.</p>
      {!data.actions.length ? <p>No actions yet. Add one for each step you want Google Ads to count.</p> : <div className="conversion-jobs">{data.actions.map(a => <article className="conversion-job" key={a.key} aria-label={`Action ${a.key}`}>
        <strong>{a.name} · <code>{a.key}</code></strong><span className="badge">{a.enabled ? "Enabled" : "Disabled"}</span>
        <small>{a.trigger === "new_lead" ? "Recorded automatically for each new attributed lead" : "Posted by your system"} · {a.counting === "once_per_click" ? "Once per click" : "Every event"} · {a.default_value_minor === null ? "No default value" : `Default ${money(a.default_value_minor)}`}</small>
        <div className="action-destinations"><span className={a.google_action_id ? "mapped" : ""}><b>Google</b>{a.google_action_id ? `Google action ${googleName(a.google_action_id) ? `${googleName(a.google_action_id)} · ` : ""}${a.google_action_id}` : "Not mapped to Google"}</span><span className={a.microsoft_goal_name ? "mapped" : ""}><b>Microsoft</b>{a.microsoft_goal_name ? `Goal ${a.microsoft_goal_name}` : "Not sent to Microsoft"}</span></div>
        {editing === a.key ? <ActionForm value={a} googleActions={googleActions} busy={busy} save={v => mutate(`/conversions/actions/${a.key}`, v, "Action saved.")} cancel={() => setEditing(null)} /> : <div className="site-actions"><button className="secondary" disabled={busy} onClick={() => setEditing(a.key)}>Edit action</button>
          {!a.google_action_id && discovery && <button className="secondary" disabled={busy || discovery.actions.account.conversion_owner !== discovery.customer} onClick={() => void mutate(`/conversion-actions/${a.key}/google-action`, { version: a.version, login: discovery.login }, "Google conversion action created or recovered and mapped. Check the saved connection on the Google Ads tab.", "POST")}>Create in Google Ads</button>}</div>}
      </article>)}</div>}
      {editing === "" ? <ActionForm value={blank} googleActions={googleActions} busy={busy} save={v => mutate(`/conversions/actions/${v.key}`, v, "Action created.")} cancel={() => setEditing(null)} /> : <div><button className="primary" disabled={busy} onClick={() => setEditing("")}>Add action</button></div>}
      {!discovery && <p className="muted">Choose and save the advertising account on the Google Ads tab to pick Google actions from a list. You can also enter a numeric action ID.</p>}
      {discovery && <p className="muted"><strong>Create in Google Ads</strong> makes a secondary (reporting only) import action named after the action key, with a 90-day click window. A retry finds the action an earlier attempt created. Prefer an existing action if you already have one.</p>}
    </section>
    </div>
    <div {...panel("events")}>
    <section className="card site-panel" aria-label="Recent pipeline events">
      <div className="section-heading"><div><span className="eyebrow">RESULTS</span><h2>Recent pipeline events</h2></div><button className="secondary" disabled={busy} onClick={() => { setError(""); void refresh().catch(e => setError(e.message)); }}>Refresh events</button></div>
      <p className="muted">Latest 100 events. Blocked events are kept with the reason they were not sent. Delivered means the ad platform processed the request; it does not prove attribution. The Logs page shows these alongside calls and forms.</p>
      {!data.events.length ? <p>No events recorded yet.</p> : <div className="conversion-jobs">{data.events.map(e => <article className="conversion-job" key={e.id} aria-label={`Event ${e.action} ${e.external_id}`}>
        <strong>{e.action} · {e.external_id}</strong><span className="event-badges"><span className={`badge delivery-${e.delivery.google}`}>Google · {label(e.delivery.google)}</span>{e.delivery.microsoft && <span className={`badge delivery-${e.delivery.microsoft.status}`}>Microsoft · {label(e.delivery.microsoft.status)}</span>}</span>
        <small>Revision {e.revision} · {e.value_minor === null ? "No value" : money(e.value_minor)} · {new Date(e.occurred_at).toLocaleString()} · matched by {label(e.matched_by)}</small>
        {e.delivery.reason && <p>{e.delivery.reason}</p>}
        {e.delivery.request_id && <small>Google request ID: <code>{e.delivery.request_id}</code></small>}
        {e.delivery.microsoft && <small>Microsoft: <strong>{label(e.delivery.microsoft.status)}</strong>{e.delivery.microsoft.reason ? ` · ${e.delivery.microsoft.reason}` : ""}{e.delivery.microsoft.tracking_id && <> · tracking ID <code>{e.delivery.microsoft.tracking_id}</code></>}</small>}
      </article>)}</div>}
    </section>
    </div>
  </>;
}
function Destination({ name, state, label, detail, summary, steps, checklist, open }: { name: string; state: "ok" | "todo" | "off"; label: string; detail: string; summary: string; steps: Step[]; checklist: string; open: () => void }) {
  return <article className={`card destination-card ${state}`} aria-label={`${name} status`}>
    <div className="destination-head"><h2>{name}</h2><span className={`state-pill ${state === "ok" ? "on" : state}`}>{label}</span></div>
    <p className="destination-detail">{detail}</p>
    <div aria-label={checklist}>{steps.length ? <ul className="destination-steps">{steps.map(s => <li key={s.step}>{s.advisory ? "Note: " : ""}{s.step}</li>)}</ul> : <p className="muted">{summary}</p>}</div>
    <div><button className="secondary" onClick={open}>Open {name === "Microsoft Advertising" ? "Microsoft Ads" : name}</button></div>
  </article>;
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
