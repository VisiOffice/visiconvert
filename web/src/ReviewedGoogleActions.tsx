import { useEffect, useState, type ReactNode } from "react";
import type { Client } from "./api";
import type { Settings } from "./Conversions";
import type { Discovery } from "./GoogleConnection";

// Manual review only: the fixed qualified-lead and booking actions. Behavior is unchanged
// from the original setup step; it now lives under "Advanced: manual review".
export function ReviewedGoogleActions({ api, discovery, settings, onChange }: { api: Client; discovery: Discovery | null; settings: Settings; onChange: () => Promise<void> }) {
  const [qualified, setQualified] = useState("");
  const [booking, setBooking] = useState("");
  const [createQualified, setCreateQualified] = useState(true);
  const [createBooking, setCreateBooking] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const customer = discovery?.customer ?? "";
  useEffect(() => {
    if (customer && customer === settings.customer_id) { setQualified(settings.qualified_action_id); setBooking(settings.booking_action_id); }
    else { setQualified(""); setBooking(""); }
  }, [customer, settings.version]);
  async function run(fn: () => Promise<void>) {
    setBusy(true); setError(""); setNotice("");
    try { await fn(); } catch (e) { setError((e as Error).message); } finally { setBusy(false); }
  }
  const section = (body: ReactNode) => <section className="card site-panel" aria-label="Manual review actions"><h2>Manual review actions</h2>
    <p className="muted">Fixed Google actions for qualified leads and bookings that you stage, validate and queue one at a time. Not needed for automatic conversions.</p>
    {error && <p className="error" role="alert">{error}</p>}{notice && <p className="notice" role="status">{notice}</p>}{body}</section>;
  if (!discovery) return section(<p>Connect Google and choose the advertising account on the Google Ads tab to set up manual review actions, or enter IDs in Advanced destination settings below.</p>);
  const { actions, login, revision } = discovery;
  const validSelection = [qualified, booking].filter(Boolean).every(id => actions.actions.some(a => a.id === id && a.eligible));
  return section(<>
    <div className="google-create-actions">
      <label className="checkbox-row"><input type="checkbox" checked={createQualified} disabled={busy} onChange={e => setCreateQualified(e.target.checked)} />Qualified leads</label>
      <p className="muted">Counts one qualified lead per ad click, with no monetary value.</p>
      <label className="checkbox-row"><input type="checkbox" checked={createBooking} disabled={busy} onChange={e => setCreateBooking(e.target.checked)} />Bookings</label>
      <p className="muted">Counts each booking and uses its recorded value and currency.</p>
      <p>New actions use a 90-day click window and start as <strong>secondary (reporting only)</strong>. After verifying delivery, choose your bidding outcome in Google Ads. Existing actions keep their current bidding setting.</p>
      <p className="muted">Creates VisiConvert actions in Google Ads and saves your selection here. Sending conversions is a separate step.</p>
      {settings.version > 0 && <p className="notice">This replaces your saved manual review mappings with the selected VisiConvert actions. Unselected outcomes will be unmapped. Existing Google actions are not deleted.</p>}
      <button className="primary" disabled={busy || (!createQualified && !createBooking) || actions.account.conversion_owner !== customer} onClick={() => void run(async () => {
        const r = await api<{ qualified_action_id: string; booking_action_id: string }>("/connections/google/actions/create", { revision, settings_version: settings.version, login, customer, qualified: createQualified, booking: createBooking });
        await onChange();
        setQualified(r.qualified_action_id); setBooking(r.booking_action_id);
        setNotice("Conversion actions created or reused and saved. Check the saved connection on the Google Ads tab. Delivery remains unchanged.");
      })}>Create conversion actions</button>
    </div>
    <details className="google-existing-actions"><summary>Use existing actions (advanced)</summary><div className="site-form">
      <p>Select an enabled import action for each outcome, leaving others unmapped. Use separate actions for leads and bookings.</p>
      {actions.has_more && <p>Showing the first 200 actions. Advanced destination settings below support other action IDs.</p>}
      {([["google-qualified", "Qualification conversion action", qualified, setQualified], ["google-booking", "Booking conversion action", booking, setBooking]] as const).map(([id, title, value, setter]) => <div className="form-field" key={id}><label htmlFor={id}>{title}</label><select id={id} disabled={busy} value={value} onChange={e => setter(e.target.value)}><option value="">Not mapped</option>{value && !actions.actions.some(a => a.id === value) && <option value={value} disabled>Saved action {value} · not in this list</option>}{actions.actions.map(a => <option key={a.id} value={a.id} disabled={!a.eligible}>{a.name} · {a.id}{a.eligible ? (a.primary ? " · primary" : " · secondary") : ` · ${a.kind} / ${a.status} / owner ${a.owner}`}</option>)}</select></div>)}
      {!actions.actions.some(a => a.eligible) && <p>No compatible existing actions were found. Use Create conversion actions above.</p>}
      {!!qualified && qualified === booking && <p className="error">Choose separate actions for qualification and bookings to avoid counting both as the same outcome.</p>}
      <div><button className="primary" disabled={busy || !validSelection || (!!qualified && qualified === booking)} onClick={() => void run(async () => { await api("/conversions/settings", { ...settings, customer_id: customer, qualified_action_id: qualified, booking_action_id: booking }, "PUT"); await onChange(); setNotice("Conversion actions saved. Check the saved connection on the Google Ads tab to finish setup. Delivery remains unchanged."); })}>Save conversion actions</button></div>
    </div></details>
  </>);
}
