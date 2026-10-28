import { useEffect, useState, type FormEvent } from "react";
import type { Client } from "./api";
type Pool = { fallback_phone: string; destination_phone: string; enabled: boolean; version: number; fallback_count: number };
type NumberRow = { id: string; label: string; phone_number: string; destination: string; site_id: string | null; enabled: boolean; used_as_fallback: boolean; reserved: boolean };
type Assignment = { id: string; phone_number: string; source: string; campaign: string; assigned_at: string; expires_at: string; reusable_at: string; revoked_at: string | null };
type Data = { pool: Pool | null; numbers: NumberRow[]; history: Assignment[] };
export function NumberPool({ api, siteId }: { api: Client; siteId: string }) {
  const [data, setData] = useState<Data | null>(null);
  const [fallback, setFallback] = useState("");
  const [destination, setDestination] = useState("");
  const [enabled, setEnabled] = useState(false);
  const [ids, setIds] = useState<string[]>([]);
  const [dedicated, setDedicated] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  function accept(v: Data) { setData(v); setFallback(v.pool?.fallback_phone || ""); setDestination(v.pool?.destination_phone || v.pool?.fallback_phone || ""); setEnabled(v.pool?.enabled || false); setIds(v.numbers.filter(n => n.site_id === siteId && n.enabled).map(n => n.id)); setDedicated(false); }
  useEffect(() => { let active = true; api<Data>(`/sites/${siteId}/number-pool`).then(v => { if (active) accept(v); }).catch(e => { if (active) setError(e.message); }); return () => { active = false; }; }, [api, siteId]); // component is keyed by site
  async function reload() { setError(""); try { accept(await api<Data>(`/sites/${siteId}/number-pool`)); } catch (e) { setError((e as Error).message); } }
  async function save(e: FormEvent) {
    e.preventDefault(); setBusy(true); setError(""); setNotice("");
    try { await api(`/sites/${siteId}/number-pool`, { fallback_phone: fallback, destination_phone: destination, enabled, version: data?.pool?.version || 0, number_ids: ids, dedicated_numbers: dedicated }, "PUT"); accept(await api<Data>(`/sites/${siteId}/number-pool`)); setNotice("Number pool saved."); }
    catch (e) { setError((e as Error).message); } finally { setBusy(false); }
  }
  return <section aria-label="Dynamic number insertion">
    <h3>Dynamic number insertion</h3>
    <p className="muted">Give each consenting visit a tracking number. Calls use the visit’s entry source as a timing-based inference. Choose the website number to replace, where calls should ring, and a dedicated pool. Register owned Twilio or SignalWire numbers under Tracking numbers first.</p>
    {error && <p className="error" role="alert">{error} Refresh the pool before retrying a conflicting edit.</p>}
    {notice && <p className="notice" role="status">{notice}</p>}
    {data && <form className="site-form" onSubmit={save}>
      <fieldset className="pool-step"><legend>1. Website number to replace</legend>
        <label>Published website number<input required pattern="\+1[0-9]{10}" placeholder="+12132795046" value={fallback} onChange={e => { const next = e.target.value; if (!destination || destination === fallback) setDestination(next); setFallback(next); }} /></label>
        <p>This is the number visitors normally see. It stays visible without consent or when the pool is unavailable. Confirm it already reaches your business.</p>
      </fieldset>
      <fieldset className="pool-step"><legend>2. Where calls should ring</legend>
        <label>Forwarding destination<input required pattern="\+1[0-9]{10}" placeholder="+12025550106" value={destination} onChange={e => setDestination(e.target.value)} /></label>
        <p>Your business or call center line. It can differ from the published number. Pool numbers must already be registered with this destination; saving the pool does not change their routing.</p>
      </fieldset>
      <fieldset className="site-fields pool-step"><legend>3. Dedicated tracking numbers</legend>
        {!data.numbers.length && <p>No real registered numbers yet.</p>}
        {data.numbers.map(n => <label key={n.id}><input type="checkbox" checked={ids.includes(n.id)} disabled={busy || !!n.site_id && n.site_id !== siteId || n.destination !== destination || n.phone_number === fallback || n.used_as_fallback} onChange={e => setIds(e.target.checked ? [...ids, n.id] : ids.filter(id => id !== n.id))} />{n.phone_number} · {n.label} · {(n.phone_number === fallback || n.used_as_fallback) ? "Website fallback — keep outside pools" : n.site_id && n.site_id !== siteId ? "Dedicated to another site" : n.destination !== destination ? `Forwards to ${n.destination}` : n.reserved ? "Reserved / cooling down" : "Available"}</label>)}
      </fieldset>
      <div className="pool-review"><h4>4. Review and test</h4><p>Replace <strong>{fallback || "your published number"}</strong> with a visitor number from <strong>{ids.length} selected number{ids.length === 1 ? "" : "s"}</strong>. Calls to pool numbers forward to <strong>{destination || "your business line"}</strong>.</p></div>
      <label className="site-toggle"><input type="checkbox" checked={enabled} onChange={e => setEnabled(e.target.checked)} />Enable number swapping</label>
      <label className="site-toggle"><input type="checkbox" checked={dedicated} onChange={e => setDedicated(e.target.checked)} />These numbers are used only for this website’s pool.</label>
      <p className="small muted">Keep Google Business Profile, printed ads, and other static campaign numbers outside this pool.</p>
      <p className="small muted">Assignments last up to 30 minutes, then numbers wait 30 more minutes before reuse. Removing a number or pausing keeps that reservation. Numbers remain dedicated to this site, including after removal.</p>
      <div className="site-actions"><button className="primary" disabled={busy || enabled && (!dedicated || !ids.length)}>Save number pool</button><button type="button" className="secondary" disabled={busy} onClick={() => void reload()}>Refresh pool</button></div>
    </form>}
    <h4>Install phone swapping</h4><p className="muted">Add <code>data-dni="true"</code> to the tracker script above. Mark phone-only links or spans with <code>data-visiconvert-phone</code>. The original text and phone link must match the published website number, not the forwarding destination. Keep icons and other words outside the marked element.</p>
    <textarea className="site-code" aria-label="Phone marker example" readOnly rows={2} value={`<a data-visiconvert-phone href="tel:${fallback || "+12025550106"}">${fallback || "+12025550106"}</a>`} />
    <p className="small muted">No consent, unavailable pool, or network failure leaves the published website number visible. Configure voice and status callbacks on each pool number in Twilio or SignalWire. Then verify the visible number swaps, call it, confirm the destination rings, and inspect Call source evidence in the inbox. Pausing is checked by visible pages every minute. Saved numbers and delayed callbacks can belong to an earlier visitor.</p>
    {data && <><p>Pool exhaustion responses: {data.pool?.fallback_count || 0}</p><details><summary>Recent number assignments ({data.history.length})</summary><ol className="inquiry-history">{data.history.map(a => <li key={a.id}><strong>{a.phone_number} · {a.source}</strong><small>{new Date(a.assigned_at).toLocaleString()} — {new Date(a.expires_at).toLocaleString()}</small><small>Reusable after {new Date(a.reusable_at).toLocaleString()}{a.revoked_at ? " · Revoked" : ""}</small>{a.campaign && <p>{a.campaign}</p>}</li>)}</ol></details></>}
  </section>;
}
export function DniEvidence({ api, leadId }: { api: Client; leadId: string }) {
  const [value, setValue] = useState<{ status: string; evidence: { confidence: string; site_name: string; warning: string; observed_at: string; number_reused?: boolean; session_entry?: Record<string, string | null> } } | null>(null);
  const [error, setError] = useState("");
  useEffect(() => { let active = true; api<typeof value>(`/leads/${leadId}/dni-evidence`).then(v => { if (active) setValue(v); }).catch(e => { if (active) setError(e.message); }); return () => { active = false; }; }, [api, leadId]);
  if (error) return <p className="error">Call source evidence: {error}</p>;
  if (!value) return null;
  return <details className="website-evidence"><summary>Call source evidence · {value.evidence.confidence}</summary><p>{value.evidence.site_name} · {value.status.replaceAll("_", " ")}</p><p className="notice">{value.evidence.warning}</p>{value.evidence.number_reused && <p>This number has previously been assigned to another visit.</p>}{value.evidence.session_entry?.source_warning && <p className="notice">{value.evidence.session_entry.source_warning}</p>}<dl>{Object.entries(value.evidence.session_entry || {}).filter(([k,v]) => v && k !== "source_warning").map(([k,v]) => <div key={k}><dt>{k.replaceAll("_", " ")}</dt><dd>{v}</dd></div>)}<div><dt>First callback observed</dt><dd>{new Date(value.evidence.observed_at).toLocaleString()}</dd></div></dl></details>;
}
