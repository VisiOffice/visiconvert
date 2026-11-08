import { useEffect, useState, type FormEvent } from "react";
import type { Client } from "./api";

type Report = { valid: boolean; message: string; dataset?: { id: string; name?: string }; test_event: { sent: boolean; trace_id?: string; test_event_code: string } | null };
type Status = { revision: number; connected: boolean; dataset_id: string | null; dataset_name: string | null; token_fingerprint: string | null; token_saved_at: string | null; match: "meta_clicks" | "all_leads"; ldu: "off" | "geolocate"; settings_version: number; validation: Report | null; checked_at: string | null; check_current: boolean };
type Action = { key: string; name: string; enabled: boolean; default_value_minor: number | null; version: number; meta_event_name?: string | null };
type Setup = { configured?: boolean; ready: boolean; summary: string; next_steps: { actor: string; step: string; advisory?: boolean }[] };
type Overview = { actions: Action[]; setup: { meta?: Setup } };

const STANDARD = ["Lead", "Schedule", "Purchase", "Contact", "SubmitApplication", "CompleteRegistration"];
const CUSTOM = "__custom__";
const when = (s: string | null) => s ? new Date(s).toLocaleString() : "";

/// Meta (Facebook and Instagram) as a third destination through the Conversions API. The
/// owner saves a dataset ID and access token here; no agent tool accepts the token.
export function MetaConnection({ api, revision, onChange }: { api: Client; revision: number; onChange: () => Promise<void> }) {
  const [status, setStatus] = useState<Status | null>(null);
  const [overview, setOverview] = useState<Overview | null>(null);
  const [editing, setEditing] = useState(false);
  const [dataset, setDataset] = useState("");
  const [token, setToken] = useState("");
  const [code, setCode] = useState("");
  const [busyCount, setBusyCount] = useState(0);
  const busy = busyCount > 0;
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  async function refresh() {
    const [s, o] = await Promise.all([api<Status>("/connections/meta"), api<Overview>("/conversions/pipeline")]);
    setStatus(s); setOverview(o); return s;
  }
  useEffect(() => { let live = true; Promise.all([api<Status>("/connections/meta"), api<Overview>("/conversions/pipeline")]).then(([s, o]) => { if (live) { setStatus(s); setOverview(o); } }).catch(e => { if (live) setError(e.message); }); return () => { live = false; }; }, [api, revision]);
  async function run(fn: () => Promise<void>) {
    setBusyCount(n => n + 1); setError(""); setNotice("");
    try { await fn(); } catch (e) { setError((e as Error).message); }
    finally { await refresh().catch(() => {}); setBusyCount(n => n - 1); }
  }
  async function changed(message: string) { await onChange(); setNotice(message); }
  function save(e: FormEvent) {
    e.preventDefault();
    if (!status) return;
    void run(async () => {
      try {
        const r = await api<{ dataset: { id: string; name?: string } }>("/connections/meta", { revision: status.revision, dataset_id: dataset.trim(), access_token: token.trim() }, "PUT");
        setEditing(false); setDataset("");
        await refresh();
        await changed(`Meta connected to dataset ${r.dataset.name ? `“${r.dataset.name}” ` : ""}(${r.dataset.id}). Map your actions to Meta events below.`);
      } finally { setToken(""); }
    });
  }
  const setup = overview?.setup.meta;
  const actions = overview?.actions.filter(a => a.enabled) ?? [];
  const report = status?.check_current ? status.validation : null;
  const form = status && (!status.connected || editing);
  return <section className="card site-panel meta-connection" aria-label="Meta Ads">
    <div className="section-heading"><div><span className="eyebrow">OPTIONAL · META (FACEBOOK AND INSTAGRAM)</span><h2>Send to Meta Ads</h2></div><button className="secondary" disabled={busy} onClick={() => void run(async () => { await refresh(); setNotice("Meta connection refreshed."); })}>Refresh Meta connection</button></div>
    <p className="muted">The same actions and events can also go to your Meta dataset (Pixel) through the Conversions API, with phone and email hashed. Delivery, automatic sending and the consent policy on the Overview tab are shared with Google and Microsoft. Only events recorded after an action is mapped are sent. <a href="https://docs.visiconvert.com/guides/meta-ads-connection/" target="_blank" rel="noreferrer">Meta setup guide</a></p>
    {error && <p className="error" role="alert">{error}</p>}{notice && <p className="notice" role="status">{notice}</p>}
    {!status ? <p>Loading Meta connection…</p> : setup && setup.configured === false ? <p>Meta needs this installation’s encryption key (<code>VISICONVERT_ENCRYPTION_KEY</code>) to store the access token. Ask the administrator to configure it.</p> : <>
      {status.connected && <div className="meta-saved">
        <dl className="site-diagnostics">
          <div><dt>Dataset</dt><dd>{status.dataset_name ? `${status.dataset_name} · ` : ""}{status.dataset_id}</dd></div>
          <div><dt>Access token</dt><dd><code>{status.token_fingerprint}</code>{status.token_saved_at && <> · saved {when(status.token_saved_at)}</>}</dd></div>
        </dl>
        {!editing && <div className="google-actions">
          <button className="secondary" disabled={busy} onClick={() => { setEditing(true); setDataset(status.dataset_id ?? ""); }}>Replace token</button>
          <button className="secondary" disabled={busy} onClick={() => void run(async () => { await api("/connections/meta", { revision: status.revision }, "DELETE"); await changed("Meta disconnected. The token is deleted and kept visitor details are cleared; event mappings are kept."); })}>Disconnect Meta</button>
        </div>}
      </div>}
      {form && <form className="site-form" onSubmit={save} aria-label="Meta connection">
        <h3>{status.connected ? "Replace the access token" : "Connect your dataset"}</h3>
        <ol className="meta-steps">
          <li>In Meta Events Manager, open your dataset (Pixel) and copy its ID.</li>
          <li>Open its <strong>Settings</strong>, find <strong>Conversions API</strong>, choose <strong>Set up with Dataset Quality API</strong> and click <strong>Generate access token</strong>. Select only this dataset.</li>
          <li>Paste both here. The token is encrypted, shown only as a fingerprint, and never given to agents.</li>
        </ol>
        <p className="muted">Meta only lets an admin or developer of the business portfolio that owns the dataset generate a token. If it refuses, ask the owner to add you, or use a dataset in a portfolio you own.</p>
        <div className="form-field"><label htmlFor="meta-dataset">Dataset (Pixel) ID</label><input id="meta-dataset" required inputMode="numeric" pattern="[0-9]{5,20}" maxLength={20} value={dataset} onChange={e => setDataset(e.target.value)} placeholder="1234567890123456" /></div>
        <div className="form-field"><label htmlFor="meta-token">Conversions API access token</label><input id="meta-token" type="password" required autoComplete="off" spellCheck={false} minLength={20} maxLength={1024} value={token} onChange={e => setToken(e.target.value)} /></div>
        <div className="site-actions"><button className="primary" disabled={busy}>{status.connected ? "Save new token" : "Connect Meta"}</button>{editing && <button type="button" className="secondary" disabled={busy} onClick={() => { setEditing(false); setToken(""); }}>Cancel</button>}</div>
        <p className="muted">VisiConvert reads the dataset with the token before saving, and refuses a token Meta rejects. Saving sends no events.</p>
      </form>}
      {status.connected && <div className="site-form">
        <h3>Meta event for each action</h3>
        <p className="muted">Pick the Meta event each action is sent as: <code>Lead</code> for new leads, <code>Schedule</code> for booked appointments, <code>Purchase</code> for won jobs (needs a value). If a CRM, form plugin or tag manager such as Zaraz already sends the same event to this dataset, turn it off there, or Meta counts it twice.</p>
        {!actions.length ? <p>Add an enabled action on the Actions tab first.</p> : <div className="goal-rows">{actions.map(a => <EventRow key={`${a.key}:${a.version}`} action={a} busy={busy}
          map={name => run(async () => { await api(`/conversion-actions/${a.key}/meta-event`, { version: a.version, event_name: name }, "PUT"); await changed(name ? `Action ${a.key} is sent to Meta as ${name}, for events recorded from now on.` : `Action ${a.key} is no longer sent to Meta.`); })} />)}</div>}
      </div>}
      {status.connected && <div className="site-form">
        <h3>Check and send a test event</h3>
        <p className="muted">Open Events Manager → the dataset → <strong>Test events</strong>, copy the test code, and send one test <code>Lead</code>. Test events never count as conversions. Leave the code empty to only check the token.</p>
        <div className="meta-check">
          <div className="form-field"><label htmlFor="meta-test-code">Test event code (optional)</label><input id="meta-test-code" maxLength={64} pattern="[A-Za-z0-9_]{1,64}" value={code} onChange={e => setCode(e.target.value)} placeholder="TEST12345" /></div>
          <button className="secondary" disabled={busy} onClick={() => void run(async () => { const r = await api<Report>("/conversion-setup/meta-check", { revision: status.revision, test_event_code: code.trim() || null }); setNotice(r.message); })}>{code.trim() ? "Send test event" : "Check Meta destination"}</button>
        </div>
      </div>}
      {report && <div className={report.valid ? "notice" : "error"}><strong>{report.valid ? report.test_event?.sent ? "Meta accepted the test event" : "Meta destination checks passed" : "Meta destination needs attention"}</strong><p>{report.message}</p>{report.test_event?.trace_id && <small>Meta trace ID <code>{report.test_event.trace_id}</code> · </small>}{status.checked_at && <small>Checked {when(status.checked_at)}</small>}</div>}
      {status.connected && <MetaSettings key={status.settings_version} status={status} busy={busy} save={(match, ldu) => run(async () => { await api("/conversion-setup/meta-settings", { settings_version: status.settings_version, match, ldu }, "PUT"); await changed(match === "all_leads" ? "Every eligible lead is now sent to Meta, matched by hashed phone and email." : "Only leads with a Meta ad click are sent to Meta."); })} />}
    </>}
    {setup && <div className={`meta-checklist ${setup.ready ? "notice" : "muted"}`} aria-label="Meta setup checklist"><strong>{setup.ready ? "Meta ready" : "Meta: still to do"}</strong>{setup.ready && <p>{setup.summary}</p>}{setup.next_steps.length > 0 && <ul>{setup.next_steps.map(s => <li key={s.step}>{s.advisory ? "Note: " : ""}{s.step}</li>)}</ul>}</div>}
  </section>;
}

/// One action's Meta event: not sent, a standard event, or a custom name.
function EventRow({ action, busy, map }: { action: Action; busy: boolean; map: (name: string | null) => Promise<void> }) {
  const saved = action.meta_event_name ?? "";
  const [choice, setChoice] = useState(saved && !STANDARD.includes(saved) ? CUSTOM : saved);
  const [custom, setCustom] = useState(saved && !STANDARD.includes(saved) ? saved : "");
  const id = `meta-event-${action.key}`;
  const draft = choice === CUSTOM ? custom.trim() : choice;
  const valid = choice !== CUSTOM || /^[A-Za-z][A-Za-z0-9_]{0,39}$/.test(draft);
  return <article className="goal-row" aria-label={`Meta event for ${action.key}`}>
    <div className="goal-row-head"><strong>{action.name}</strong><code>{action.key}</code><span className={`state-pill ${saved ? "on" : ""}`}>{saved ? `Sending as ${saved}` : "Not sent"}</span></div>
    <div className="form-field">
      <label htmlFor={id}>Meta event</label>
      <select id={id} className="goal-select" disabled={busy} value={choice} onChange={e => setChoice(e.target.value)}>
        <option value="">Don’t send to Meta</option>
        {STANDARD.map(n => <option key={n} value={n}>{n}</option>)}
        <option value={CUSTOM}>Custom event name…</option>
      </select>
    </div>
    {choice === CUSTOM && <div className="form-field"><label htmlFor={`${id}-custom`}>Custom event name</label><input id={`${id}-custom`} maxLength={40} value={custom} onChange={e => setCustom(e.target.value)} placeholder="QualifiedLead" /></div>}
    {choice === "Purchase" && action.default_value_minor === null && <p className="muted">Meta needs a value for Purchase. Events without one are held; post the job value with each event, or set a default value on the Actions tab.</p>}
    {draft !== saved && <div className="site-actions">
      <button className="primary" disabled={busy || !valid} onClick={() => void map(draft || null)}>{draft ? "Use this event" : "Stop sending to Meta"}</button>
      <button className="secondary" disabled={busy} onClick={() => { setChoice(saved && !STANDARD.includes(saved) ? CUSTOM : saved); setCustom(saved && !STANDARD.includes(saved) ? saved : ""); }}>Cancel</button>
    </div>}
  </article>;
}

/// Which leads Meta receives and Limited Data Use. Both change what is shared with Meta.
function MetaSettings({ status, busy, save }: { status: Status; busy: boolean; save: (match: Status["match"], ldu: Status["ldu"]) => Promise<void> }) {
  const [match, setMatch] = useState(status.match);
  const [ldu, setLdu] = useState(status.ldu);
  const dirty = match !== status.match || ldu !== status.ldu;
  return <form className="site-form" aria-label="Meta sharing" onSubmit={e => { e.preventDefault(); void save(match, ldu); }}>
    <h3>What is shared with Meta</h3>
    <fieldset className="key-group"><legend>Which leads are sent</legend>
      <label className="site-toggle"><input type="radio" name="meta-match" checked={match === "meta_clicks"} onChange={() => setMatch("meta_clicks")} /><span>Only leads from a Meta ad click (recommended): the visit had an <code>fbclid</code>, or a Facebook or Instagram campaign tag with the Pixel’s browser ID</span></label>
      <label className="site-toggle"><input type="radio" name="meta-match" checked={match === "all_leads"} onChange={() => setMatch("all_leads")} /><span>Every eligible lead, including calls and leads with no website visit; Meta matches them by hashed phone and email</span></label>
    </fieldset>
    <fieldset className="key-group"><legend>US state privacy laws</legend>
      <label className="site-toggle"><input type="radio" name="meta-ldu" checked={ldu === "off"} onChange={() => setLdu("off")} /><span>Off: send no data processing options</span></label>
      <label className="site-toggle"><input type="radio" name="meta-ldu" checked={ldu === "geolocate"} onChange={() => setLdu("geolocate")} /><span>Limited Data Use: Meta applies state privacy laws by the visitor’s location</span></label>
    </fieldset>
    {dirty && <div className="site-actions"><button className="primary" disabled={busy}>Save sharing settings</button><button type="button" className="secondary" disabled={busy} onClick={() => { setMatch(status.match); setLdu(status.ldu); }}>Cancel</button></div>}
  </form>;
}
