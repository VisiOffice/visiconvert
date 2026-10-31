import { useEffect, useRef, useState } from "react";
import type { Client } from "./api";
import type { Settings } from "./Conversions";

type Check = { valid: boolean; message: string; settings_version: number; account?: { name: string }; actions?: { name: string; primary: boolean }[] };
type Status = { configured: boolean; connected: boolean; reconnect_required: boolean; revision: number; redirect_uri: string; validation: Check | null; checked_at: string | null };
type Account = { id: string; name: string; manager: boolean; status: string };
type Action = { id: string; name: string; eligible: boolean; kind: string; status: string; primary: boolean; owner: string };
type Accounts = { logins: string[]; accounts: Account[]; has_more: boolean };
type Actions = { account: { id: string; name: string; conversion_owner: string }; actions: Action[]; has_more: boolean };

export function GoogleConnection({ api, settings, onChange }: { api: Client; settings: Settings; onChange: () => Promise<void> }) {
  const [status, setStatus] = useState<Status | null>(null);
  const [logins, setLogins] = useState<string[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [login, setLogin] = useState("");
  const [customer, setCustomer] = useState(settings.customer_id);
  const [actions, setActions] = useState<Actions | null>(null);
  const [qualified, setQualified] = useState("");
  const [booking, setBooking] = useState("");
  const [moreAccounts, setMoreAccounts] = useState(false);
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const cleanup = useRef<() => void>(() => {});
  async function refresh() { const s = await api<Status>("/connections/google"); setStatus(s); return s; }
  useEffect(() => { let live = true; api<Status>("/connections/google").then(s => { if (live) setStatus(s); }).catch(e => { if (live) setError(e.message); }); return () => { live = false; cleanup.current(); }; }, [api]);
  // Saving the destination elsewhere invalidates the provider check immediately.
  useEffect(() => { void refresh().catch(e => setError(e.message)); }, [settings.version]);
  async function run(fn: () => Promise<void>) {
    setBusy(true); setError(""); setNotice("");
    try { await fn(); } catch (e) { setError((e as Error).message); }
    finally { await refresh().catch(() => {}); setBusy(false); }
  }
  function connect() {
    if (!status) return;
    // Open synchronously so normal popup blockers allow the consent window.
    const popup = window.open("about:blank", "visiconvert-google", "popup,width=640,height=760");
    if (!popup) { setError("Allow popups for this site, then connect Google again."); return; }
    cleanup.current(); setPending(true);
    const revision = status.revision;
    let stopped = false; let timer: ReturnType<typeof setTimeout> | undefined;
    const stop = () => { stopped = true; clearTimeout(timer); window.removeEventListener("message", message); setPending(false); };
    const message = (event: MessageEvent) => {
      if (event.origin !== window.location.origin || event.source !== popup || event.data?.type !== "visiconvert-google-result") return;
      stop();
      void refresh().then(s => {
        if (s.connected && s.revision > revision) { setNotice("Google connected. Browse accounts and validate your destination."); setError(""); }
        else setError("Google was not connected. Try again and grant both requested permissions. Ask your administrator to check the OAuth configuration if it keeps failing.");
      }).catch(e => setError(e.message));
    };
    cleanup.current = stop; window.addEventListener("message", message);
    void run(async () => {
      try {
        const result = await api<{ authorization_url: string }>("/connections/google/start", { revision });
        const u = new URL(result.authorization_url);
        if (u.origin !== "https://accounts.google.com") throw new Error("Unexpected Google authorization address.");
        popup.location.href = u.href;
        setNotice("Finish connecting in the Google window. You can also refresh the connection here afterward.");
        let tries = 0;
        const poll = async () => {
          if (stopped) return;
          try { const s = await refresh(); if (s.connected && s.revision > revision) { stop(); popup.close(); setNotice("Google connected. Browse accounts and validate your destination."); return; } }
          catch { /* The explicit refresh control remains available. */ }
          if (++tries < 300) timer = setTimeout(() => void poll(), 2000); else { stop(); setNotice("Connection window expired. Refresh the connection or try again."); }
        };
        timer = setTimeout(() => void poll(), 2000);
      } catch (e) { stop(); popup.close(); throw e; }
    });
  }
  async function browse() {
    const r = await api<Accounts>("/connections/google/accounts"); setLogins(r.logins); setLogin(""); setAccounts([]); setActions(null);
    setNotice(r.logins.length ? "Choose the account that gives you access to Google Ads." : "Google connected, but this Google user has no directly accessible Ads accounts.");
  }
  async function chooseLogin(id: string) {
    setLogin(id); setActions(null); setAccounts([]); setCustomer("");
    if (!id) return;
    const r = await api<Accounts>(`/connections/google/accounts?login=${encodeURIComponent(id)}`);
    setAccounts(r.accounts); setMoreAccounts(r.has_more);
  }
  const validation = status?.validation?.settings_version === settings.version ? status.validation : null;
  return <section className="card site-panel google-connection">
    <div className="section-heading"><div><span className="eyebrow">GOOGLE CONNECTION</span><h2>Connect Google Ads</h2></div><button className="secondary" disabled={busy} onClick={() => void run(async () => { await refresh(); setNotice("Connection refreshed."); })}>Refresh Google connection</button></div>
    <p className="muted">Connect to browse accounts and check conversion actions. Sending requires separate event validation and explicit queuing below.</p>
    {error && <p className="error" role="alert">{error}</p>}{notice && <p className="notice" role="status">{notice}</p>}
    {!status ? <p>Loading Google connection…</p> : !status.configured ? <>
      <p>Google connection is not configured on this installation. Your administrator must add the Google OAuth application credentials and enable the Google Ads and Data Manager APIs.</p>
      <details><summary>Administrator setup</summary><p>Use a Web application OAuth client with this exact authorized redirect URI:</p><code className="google-callback">{status.redirect_uri}</code><p>Set <code>VISICONVERT_GOOGLE_CLIENT_ID</code> and <code>VISICONVERT_GOOGLE_CLIENT_SECRET</code> in the server’s private configuration, with its existing encryption key. See the Google connection guide in the installation documentation.</p></details>
    </> : <>
      <p><strong>{status.connected ? "Google connected" : status.reconnect_required ? "Reconnect Google after the application configuration change" : "Google not connected"}</strong></p>
      <div className="google-actions"><button className="primary" disabled={busy || pending} onClick={connect}>{status.connected ? "Reconnect Google" : "Connect Google"}</button>
        {(status.connected || status.revision > 0 || pending) && <button className="secondary" disabled={busy} onClick={() => void run(async () => { await api("/connections/google", { revision: status.revision }, "DELETE"); cleanup.current(); setActions(null); setAccounts([]); setLogins([]); setLogin(""); setNotice("Google disconnected locally. You can also remove VisiConvert access in your Google Account permissions."); })}>Disconnect Google</button>}
        {status.connected && <button className="secondary" disabled={busy} onClick={() => void run(browse)}>Browse Google Ads</button>}</div>
      {status.connected && logins.length > 0 && <div className="site-form">
        <div className="form-field"><label htmlFor="google-login">Google account for access</label><select id="google-login" disabled={busy} value={login} onChange={e => void run(() => chooseLogin(e.target.value))}><option value="">Select an account</option>{logins.map(id => <option key={id} value={id}>{id}</option>)}</select></div>
        {login && <><div className="form-field"><label htmlFor="google-customer">Advertising account ID</label><input id="google-customer" list="google-accounts" disabled={busy} inputMode="numeric" maxLength={10} value={customer} onChange={e => { setCustomer(e.target.value); setActions(null); }} /><datalist id="google-accounts">{accounts.map(c => <option value={c.id} key={c.id}>{c.name}{c.manager ? " (manager)" : ""} · {c.status}</option>)}</datalist></div>
          {moreAccounts && <p>Showing the first 200 accounts. You can enter another account ID accessible through this login account.</p>}
          <div><button className="secondary" disabled={busy || !/^\d{10}$/.test(customer)} onClick={() => void run(async () => { const r = await api<Actions>(`/connections/google/actions?login=${encodeURIComponent(login)}&customer=${encodeURIComponent(customer)}`); setActions(r); setQualified(""); setBooking(""); })}>Load conversion actions</button></div>
        </>}
        {actions && <><p>Actions in <strong>{actions.account.name || customer}</strong>. Only enabled import actions owned by this account can be selected.</p>
          {actions.account.conversion_owner && actions.account.conversion_owner !== customer && <p className="notice">Conversion tracking is owned by {actions.account.conversion_owner}. Select that account to validate its actions.</p>}
          {actions.has_more && <p>Showing the first 200 actions. Use the destination fields below for other action IDs.</p>}
          {([["google-qualified", "Qualification conversion action", qualified, setQualified], ["google-booking", "Booking conversion action", booking, setBooking]] as const).map(([id, title, value, setter]) => <div className="form-field" key={id}><label htmlFor={id}>{title}</label><select id={id} disabled={busy} value={value} onChange={e => setter(e.target.value)}><option value="">Not mapped</option>{actions.actions.map(a => <option key={a.id} value={a.id} disabled={!a.eligible}>{a.name} · {a.id}{a.eligible ? (a.primary ? " · primary" : " · secondary") : ` · ${a.kind} / ${a.status} / owner ${a.owner}`}</option>)}</select></div>)}
          {!actions.actions.some(a => a.eligible) && <p>No enabled import actions owned by this account were found. Create the required action in Google Ads, then load actions again.</p>}
          <div><button className="primary" disabled={busy || (!qualified && !booking) || (!!qualified && qualified === booking)} onClick={() => void run(async () => { await api("/conversions/settings", { ...settings, customer_id: customer, qualified_action_id: qualified, booking_action_id: booking }, "PUT"); await onChange(); setNotice("Selected actions saved. Validate the saved destination next."); })}>Use selected actions</button></div>
        </>}
      </div>}
      {status.connected && <div className="google-actions"><button className="secondary" disabled={busy || !login || settings.version === 0} onClick={() => void run(async () => { const r = await api<Check>("/connections/google/validate", { revision: status.revision, settings_version: settings.version, login }); setNotice(r.message); })}>Validate saved destination</button><p className="muted">Browse Google Ads and choose an access account before validating the saved destination.</p></div>}
    </>}
    {validation && <div className={validation.valid ? "notice" : "error"}><strong>{validation.valid ? "Destination checks passed" : "Destination needs attention"}</strong><p>{validation.message}</p>{status?.checked_at && <small>Checked {new Date(status.checked_at).toLocaleString()}</small>}</div>}
  </section>;
}
