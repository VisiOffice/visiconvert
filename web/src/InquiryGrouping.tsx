import { useEffect, useState, type FormEvent } from "react";
import { label, money, type Client, type Inquiry, type Lead } from "./api";
import { DniEvidence } from "./NumberPool";
import { WebsiteEvidence } from "./Sites";

type History = { id: number; kind: string; actor: string; created_at: string; payload: { reason?: string; source_inquiry_id?: string; source_snapshot?: { notes: string; qualification: string }; booking?: { external_id: string; status: string; amount_minor: number }; interaction_id?: string } };
type Detail = { inquiry: Inquiry; interactions: Lead[]; history: History[] };

function HistoryList({ items }: { items: History[] }) {
  return <ol className="inquiry-history">{items.map(e => <li key={e.id}>
    <strong>{label(e.kind)}</strong><small>{new Date(e.created_at).toLocaleString()} · {e.actor}</small>
    {e.payload.reason && <p>{e.payload.reason}</p>}
    {e.payload.booking && <p>{e.payload.booking.external_id} · {e.payload.booking.status} · {money(e.payload.booking.amount_minor)}</p>}
    {e.payload.source_snapshot && <details><summary>Previous inquiry review</summary><p>{label(e.payload.source_snapshot.qualification)}</p><p className="preserve-lines">{e.payload.source_snapshot.notes || "No notes"}</p></details>}
  </li>)}</ol>;
}

export function InquiryGrouping({ inquiry, api, disabled, changed }: { inquiry: Inquiry; api: Client; disabled: boolean; changed: (id: string) => Promise<void> }) {
  const [detail, setDetail] = useState<Detail | null>(null);
  const [choices, setChoices] = useState<Inquiry[]>([]);
  const [target, setTarget] = useState("");
  const [search, setSearch] = useState("");
  const [reason, setReason] = useState("");
  const [split, setSplit] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [earlier, setEarlier] = useState<History[] | null>(null);
  useEffect(() => {
    let active = true; setError(""); setDetail(null); setTarget(""); setSplit(""); setReason(""); setEarlier(null);
    Promise.all([api<Detail>(`/inquiries/${inquiry.id}`), api<Inquiry[]>("/inquiries")]).then(([d, c]) => { if (active) { setDetail(d); setChoices(c); } }).catch(e => { if (active) setError(e.message); });
    return () => { active = false; };
  }, [api, inquiry]);
  const targetInquiry = choices.find(c => c.id === target);
  const blocked = disabled || busy || !detail || detail.inquiry.version !== inquiry.version;
  async function combine(e: FormEvent) {
    e.preventDefault(); if (blocked || !detail || !targetInquiry) return;
    setBusy(true); setError("");
    try {
      const result = await api<{ id: string }>(`/inquiries/${inquiry.id}/combine`, { target_id: target, version: detail.inquiry.version, target_version: targetInquiry.version, reason });
      await changed(result.id);
    } catch (e) { setError((e as Error).message); } finally { setBusy(false); }
  }
  async function separate(e: FormEvent) {
    e.preventDefault(); if (blocked || !detail) return;
    setBusy(true); setError("");
    try {
      const result = await api<{ id: string }>(`/inquiries/${inquiry.id}/split`, { interaction_id: split, version: detail.inquiry.version, reason });
      await changed(result.id);
    } catch (e) { setError((e as Error).message); } finally { setBusy(false); }
  }
  return <section className="inquiry-grouping" aria-label="Inquiry interactions">
    <h3>{inquiry.interaction_count} {inquiry.interaction_count === 1 ? "interaction" : "interactions"}</h3>
    <p className="small muted">Each call and form keeps its original source. The inquiry’s reporting source is the interaction marked below.</p>
    {detail && detail.inquiry.version !== inquiry.version && <p className="notice">This inquiry changed. Refresh data before grouping it.</p>}
    {detail?.interactions.map(interaction => <article className="interaction" key={interaction.id}>
      <div className="interaction-heading"><strong>{label(interaction.channel)} · {interaction.name}</strong>{interaction.id === inquiry.primary_lead_id && <span className="chip">Reporting source</span>}</div>
      <p>{interaction.contact}</p><small>{new Date(interaction.created_at).toLocaleString()} · {interaction.source}{interaction.campaign ? ` · ${interaction.campaign}` : ""}</small>
      {interaction.channel === "call" && <p className="small">{label(interaction.call_status || "unknown")} · {interaction.tracking_number} → {interaction.destination}{interaction.duration_seconds != null ? ` · ${interaction.duration_seconds}s` : ""}</p>}
      {interaction.channel === "call" && <small>A destination answer may be voicemail or an IVR.</small>}
      {interaction.booked_minor > 0 && <p className="small">{money(interaction.booked_minor)} booked on this interaction</p>}
      {interaction.notes && <details><summary>Original interaction notes</summary><p className="preserve-lines">{interaction.notes}</p></details>}
      {interaction.channel === "call" && <DniEvidence api={api} leadId={interaction.id} />}
      {interaction.channel === "form" && <WebsiteEvidence api={api} leadId={interaction.id} />}
      {inquiry.interaction_count > 1 && <button className="text-button" disabled={blocked} onClick={() => { setSplit(interaction.id); setReason(""); setTarget(""); }}>Separate this interaction</button>}
    </article>)}
    {split ? <form onSubmit={separate} className="grouping-form">
      <h4>Create a separate inquiry</h4><p className="small">This interaction and its recorded bookings will move to a new, unreviewed inquiry with empty notes. {split === inquiry.primary_lead_id && "The remaining inquiry will use its oldest remaining interaction as its reporting source."}</p>
      <label>Reason for separating<textarea required maxLength={500} value={reason} onChange={e => setReason(e.target.value)} /></label>
      <div className="site-actions"><button className="secondary" disabled={blocked || !reason.trim()}>Create separate inquiry</button><button className="text-button" type="button" onClick={() => setSplit("")}>Cancel separation</button></div>
    </form> : <details className="grouping-controls"><summary>Combine with another inquiry</summary>
      <p className="small">Combine only interactions about the same opportunity. Shared contact details do not prove they are the same person or booking.</p>
      <form onSubmit={combine}>
        <label>Find an inquiry<input value={search} onChange={e => { setSearch(e.target.value); setTarget(""); }} placeholder="Name, contact, source, campaign…" /></label>
        <label htmlFor="inquiry-destination">Destination inquiry</label><select id="inquiry-destination" required value={target} onChange={e => setTarget(e.target.value)}><option value="">Choose an inquiry</option>{choices.filter(c => c.id !== inquiry.id && c.is_demo === inquiry.is_demo && c.search_text.toLowerCase().includes(search.toLowerCase())).map(c => <option value={c.id} key={c.id}>{c.name} · {c.contact} · {c.source} · {new Date(c.created_at).toLocaleDateString()} · {c.id.slice(0, 8)}</option>)}</select>
        {targetInquiry && <div className="notice"><strong>Keep {targetInquiry.name} as the inquiry</strong><p>Reporting source: {targetInquiry.source}. Qualification: {label(targetInquiry.qualification)}. {targetInquiry.interaction_count + inquiry.interaction_count} interactions after combining.</p><p>The destination’s notes stay in place. This inquiry’s notes remain in history. Existing booking values are counted once.</p></div>}
        <label>Reason for combining<textarea required maxLength={500} value={reason} onChange={e => setReason(e.target.value)} /></label>
        <button className="secondary" disabled={blocked || !targetInquiry || !reason.trim()}>Combine inquiries</button>
        <small>Choose from the latest 500 inquiries. Save any review edits before combining.</small>
      </form>
    </details>}
    {detail && <details className="grouping-history"><summary>Inquiry history</summary><HistoryList items={detail.history} />
      {Array.from(new Set(detail.history.filter(e => e.kind === "combined_from" || e.kind === "split_from").map(e => e.payload.source_inquiry_id).filter((v): v is string => !!v && v !== inquiry.id))).map(id => <button key={id} type="button" className="text-button" onClick={() => { void api<History[]>(`/inquiries/${id}/history`).then(setEarlier).catch(e => setError(e.message)); }}>Earlier inquiry history · {id.slice(0, 8)}</button>)}
      {earlier && <div><h4>Earlier inquiry history</h4><HistoryList items={earlier} /></div>}
      <small>Showing the latest 100 events per inquiry.</small>
    </details>}
    {error && <div><p role="alert" className="error">{error}</p><button type="button" className="secondary" disabled={busy || disabled} onClick={() => { void changed(inquiry.id).then(async () => { setChoices(await api<Inquiry[]>("/inquiries")); setTarget(""); setError(""); }).catch(e => setError(e.message)); }}>Reload inquiry before retrying</button></div>}
  </section>;
}
