import { useEffect, useState, type FormEvent } from "react";
import { NumberPool } from "./NumberPool";
import type { Client } from "./api";

type Site = {
  id: string; name: string; origin: string; allowed_fields: string[]; enabled: boolean; version: number;
  last_session_at: string | null; last_form_at: string | null; session_count: number; form_count: number;
};
type SiteList = { sites: Site[]; tracker_url: string };
const fields = ["name", "email", "phone", "company"];
const seen = (value: string | null) => value ? new Date(value).toLocaleString() : "Not received yet";
const blank = { name: "", origin: "", allowed_fields: ["name", "email", "phone"], enabled: true, version: 0 };

export function Sites({ api }: { api: Client }) {
  const [data, setData] = useState<SiteList>({ sites: [], tracker_url: "" });
  const [selected, setSelected] = useState<string | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState(blank);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  async function refresh() { setData(await api<SiteList>("/sites")); }
  useEffect(() => { let active = true; api<SiteList>("/sites").then(v => { if (active) setData(v); }).catch(e => { if (active) setError(e.message); }); return () => { active = false; }; }, [api]);
  async function save(e: FormEvent) {
    e.preventDefault(); setBusy(true); setError(""); setNotice("");
    try {
      const result = await api<{ id: string }>(editing ? `/sites/${editing}` : "/sites", draft, editing ? "PATCH" : "POST");
      await refresh(); setSelected(result.id); setEditing(null); setDraft(blank); setNotice("Site saved. Follow the installation steps below.");
    } catch (e) { setError((e as Error).message); } finally { setBusy(false); }
  }
  const site = data.sites.find(s => s.id === selected) || data.sites[0];
  const snippet = site ? `<script defer src="${data.tracker_url}" data-site="${site.id}" referrerpolicy="no-referrer"></script>` : "";
  const hook = `// After your consent manager grants tracking consent:\nawait window.VisiConvert.setConsent(true);\n\n// Only after your form backend confirms a successful submission:\nawait window.VisiConvert.trackForm({\n  submissionId: result.submissionId, // stable ID from your backend\n  fields: { name: values.name, email: values.email, phone: values.phone }\n});\n\n// When consent is withdrawn:\nawait window.VisiConvert.setConsent(false);`;
  return <div className="sites-layout">
    {error && <p className="error" role="alert">{error}</p>}
    {notice && <p className="notice" role="status">{notice}</p>}
    <section className="card site-panel">
      <div className="section-heading"><div><span className="eyebrow">WEBSITE CAPTURE</span><h2>{editing ? "Edit website" : "Add a website"}</h2></div></div>
      <p className="muted">Connect visits and successful forms to leads. Tracking stays off until your consent integration enables it.</p>
      <form onSubmit={save} className="site-form">
        <div className="form-field"><label htmlFor="site-name">Website name</label><input id="site-name" required maxLength={100} value={draft.name} onChange={e => setDraft({ ...draft, name: e.target.value })} placeholder="My business" /></div>
        <div className="form-field"><label htmlFor="site-origin">Website origin</label><input id="site-origin" required type="url" value={draft.origin} onChange={e => setDraft({ ...draft, origin: e.target.value })} placeholder="https://example.com" /><small>Exact origin, including www if used. Add other origins as separate sites.</small></div>
        <fieldset className="site-fields"><legend>Allowed form fields</legend>{fields.map(field => <label key={field}><input type="checkbox" checked={draft.allowed_fields.includes(field)} onChange={e => setDraft({ ...draft, allowed_fields: e.target.checked ? [...draft.allowed_fields, field] : draft.allowed_fields.filter(f => f !== field) })} />{field}</label>)}</fieldset>
        <label className="site-toggle"><input type="checkbox" checked={draft.enabled} onChange={e => setDraft({ ...draft, enabled: e.target.checked })} />Collection enabled</label>
        <div className="site-actions"><button className="primary" disabled={busy}>{busy ? "Saving…" : editing ? "Save website" : "Add website"}</button>{editing && <button type="button" className="secondary" onClick={() => { setEditing(null); setDraft(blank); }}>Cancel edit</button>}</div>
      </form>
    </section>
    <section className="card site-panel">
      <div className="section-heading"><div><span className="eyebrow">YOUR WEBSITES</span><h2>Installation & diagnostics</h2></div><button className="secondary" onClick={() => { setError(""); void refresh().catch(e => setError(e.message)); }}>Refresh diagnostics</button></div>
      {!site ? <p className="muted">Add your first website to get its installation snippet.</p> : <>
        <div className="form-field"><label htmlFor="site-select">Website</label><select id="site-select" value={site.id} onChange={e => setSelected(e.target.value)}>{data.sites.map(s => <option key={s.id} value={s.id}>{s.name}</option>)}</select></div>
        <div className="site-actions"><strong>{site.origin}</strong><span className="badge">{site.enabled ? "Collection enabled" : "Collection paused"}</span><button className="secondary" onClick={() => { setEditing(site.id); setDraft({ name: site.name, origin: site.origin, allowed_fields: site.allowed_fields, enabled: site.enabled, version: site.version }); }}>Edit website</button></div>
        <dl className="site-diagnostics"><div><dt>Visits received</dt><dd>{site.session_count}</dd><small>{seen(site.last_session_at)}</small></div><div><dt>Forms received</dt><dd>{site.form_count}</dd><small>{seen(site.last_form_at)}</small></div></dl>
        <h3>1. Install the snippet</h3><p className="muted">Add this to your website’s HTML. It contains a public site ID, never your workspace key.</p>
        {data.tracker_url.startsWith("http:") && <p className="notice">Use HTTPS on your VisiConvert server before installing this on a public website. HTTP is suitable only for local testing.</p>}
        <textarea className="site-code" aria-label="Installation snippet" readOnly rows={3} value={snippet} />
        <h3>2. Connect consent and form success</h3><p className="muted">Run this integration after the script loads (the <code>visiconvert:ready</code> event). Use your form backend’s success response. A click or submit event is not a confirmed submission.</p>
        <textarea className="site-code" aria-label="Form integration example" readOnly rows={15} value={hook} />
        <p className="muted">Only allowed fields are sent. Catch tracking errors separately so they never block your customer’s form submission. Retries must use the same submission ID and field values.</p>
        <h3>3. Verify a complete visit</h3><p className="muted">Open a campaign URL, grant consent, and complete a form. Refresh these diagnostics, then open the lead inbox to inspect its source evidence. No visits yet? Check the exact origin, consent hook, browser blockers, and your website’s script-src/connect-src policy.</p>
        <p className="notice">Attribution describes the session’s entry source, not proof that an ad caused a booking. Sessions last 30 minutes; this release does not track across devices.</p>
        <NumberPool key={site.id} api={api} siteId={site.id} />
      </>}
    </section>
  </div>;
}

type Evidence = { site_name: string; session_started_at: string; submitted_at: string; evidence: Record<string, string | null>; fields: Record<string, string> };
export function WebsiteEvidence({ api, leadId }: { api: Client; leadId: string }) {
  const [value, setValue] = useState<Evidence | null>(null);
  const [error, setError] = useState("");
  useEffect(() => { let active = true; setValue(null); setError(""); api<Evidence | null>(`/leads/${leadId}/website-evidence`).then(v => { if (active) setValue(v); }).catch(e => { if (active) setError(e.message); }); return () => { active = false; }; }, [api, leadId]);
  if (error) return <p className="error" role="alert">Source evidence: {error}</p>;
  if (!value) return null;
  return <details className="website-evidence"><summary>Website source evidence</summary><p>{value.site_name} · Browser-reported session entry</p><dl>{Object.entries(value.evidence).filter(([, v]) => v).map(([k, v]) => <div key={k}><dt>{k.replaceAll("_", " ")}</dt><dd>{v}</dd></div>)}<div><dt>Session started</dt><dd>{seen(value.session_started_at)}</dd></div></dl>{Object.keys(value.fields).length > 0 && <><h4>Captured fields</h4><dl>{Object.entries(value.fields).map(([k,v]) => <div key={k}><dt>{k}</dt><dd>{v}</dd></div>)}</dl></>}<small>Consent was reported by the website. This does not verify the visitor’s identity or submission authenticity.</small></details>;
}
