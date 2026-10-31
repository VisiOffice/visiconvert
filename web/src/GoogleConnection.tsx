import { useEffect, useRef, useState } from "react";
import type { Client } from "./api";
import type { Settings } from "./Conversions";

type Check = { valid: boolean; message: string; settings_version: number; login_customer_id?: string; account?: { name: string }; actions?: { name: string; primary: boolean }[] };
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
  const [createQualified, setCreateQualified] = useState(true);
  const [createBooking, setCreateBooking] = useState(true);
  const [moreAccounts, setMoreAccounts] = useState(false);
  const [busyCount, setBusyCount] = useState(0);
  const busy = busyCount > 0;
  const [accountsLoaded, setAccountsLoaded] = useState(false);
  const [manualCustomer, setManualCustomer] = useState("");
  const discovery = useRef(0);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const cleanup = useRef<() => void>(() => {});
  async function refresh() { const s = await api<Status>("/connections/google"); setStatus(s); return s; }
  useEffect(() => { let live = true; api<Status>("/connections/google").then(s => { if (live) setStatus(s); }).catch(e => { if (live) setError(e.message); }); return () => { live = false; cleanup.current(); discovery.current++; }; }, [api]);
  // Saving the destination elsewhere invalidates the provider check immediately.
  useEffect(() => { void refresh().catch(e => setError(e.message)); }, [settings.version]);
  async function run(fn: () => Promise<void>) {
    setBusyCount(n => n + 1); setError(""); setNotice("");
    try { await fn(); } catch (e) { setError((e as Error).message); }
    finally { await refresh().catch(() => {}); setBusyCount(n => n - 1); }
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
        if (s.connected && s.revision > revision) { setNotice("Google sign-in complete. Choose the advertising account you want to connect below."); setError(""); }
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
          try { const s = await refresh(); if (s.connected && s.revision > revision) { stop(); popup.close(); setNotice("Google sign-in complete. Choose the advertising account you want to connect below."); return; } }
          catch { /* The explicit refresh control remains available. */ }
          if (++tries < 300) timer = setTimeout(() => void poll(), 2000); else { stop(); setNotice("Connection window expired. Refresh the connection or try again."); }
        };
        timer = setTimeout(() => void poll(), 2000);
      } catch (e) { stop(); popup.close(); throw e; }
    });
  }
  // Show the next setup step as soon as OAuth succeeds, including on a return visit.
  useEffect(() => {
    if (status?.connected) void run(browse);
    else { discovery.current++; setLogins([]); setAccounts([]); setActions(null); setAccountsLoaded(false); setLogin(""); }
    return () => { discovery.current++; };
  }, [api, status?.connected, status?.revision]);
  async function browse() {
    const request = ++discovery.current;
    setAccountsLoaded(false); setLogins([]); setLogin(""); setAccounts([]); setActions(null); setMoreAccounts(false);
    const r = await api<Accounts>("/connections/google/accounts");
    if (request !== discovery.current) return;
    setLogins(r.logins); setAccountsLoaded(true);
    const savedLogin = status?.validation?.login_customer_id;
    if (r.logins.length === 1) await chooseLogin(r.logins[0]);
    else if (savedLogin && r.logins.includes(savedLogin)) await chooseLogin(savedLogin);
  }
  async function chooseLogin(id: string) {
    const request = ++discovery.current;
    setLogin(id); setActions(null); setAccounts([]); setCustomer(""); setManualCustomer(""); setMoreAccounts(false);
    setQualified(""); setBooking("");
    if (!id) return;
    const r = await api<Accounts>(`/connections/google/accounts?login=${encodeURIComponent(id)}`);
    if (request !== discovery.current) return;
    setAccounts(r.accounts); setMoreAccounts(r.has_more);
    if (r.accounts.some(a => a.id === settings.customer_id)) await chooseCustomer(settings.customer_id, id);
  }
  async function chooseCustomer(id: string, access = login) {
    const request = ++discovery.current;
    setCustomer(id); setActions(null); setQualified(""); setBooking("");
    if (!id) return;
    const r = await api<Actions>(`/connections/google/actions?login=${encodeURIComponent(access)}&customer=${encodeURIComponent(id)}`);
    if (request !== discovery.current) return;
    setActions(r);
    if (id === settings.customer_id) { setQualified(settings.qualified_action_id); setBooking(settings.booking_action_id); }
  }
  const accountId = (id: string) => id.replace(/^(\d{3})(\d{3})(\d{4})$/, "$1-$2-$3");
  const matchesSaved = customer === settings.customer_id && qualified === settings.qualified_action_id && booking === settings.booking_action_id;
  const validSelection = !!actions && [qualified, booking].filter(Boolean).every(id => actions.actions.some(a => a.id === id && a.eligible));
  const validation = status?.validation?.settings_version === settings.version ? status.validation : null;
  return <section className="card site-panel google-connection">
    <div className="section-heading"><div><span className="eyebrow">GOOGLE ADS SETUP</span><h2>Connect Google Ads</h2></div><button className="secondary" disabled={busy} onClick={() => void run(async () => { await refresh(); setNotice("Connection refreshed."); })}>Refresh Google connection</button></div>
    <p className="muted">Sign in with Google, choose the account that runs your ads, then select the outcomes to track. Connecting an account does not send conversions.</p>
    {error && <p className="error" role="alert">{error}</p>}{notice && <p className="notice" role="status">{notice}</p>}
    {!status ? <p>Loading Google connection…</p> : !status.configured ? <>
      <p>Google connection is not configured on this installation. Your administrator must add the Google OAuth application credentials and enable the Google Ads and Data Manager APIs.</p>
      <details><summary>Administrator setup</summary><p>Use a Web application OAuth client with this exact authorized redirect URI:</p><code className="google-callback">{status.redirect_uri}</code><p>Set <code>VISICONVERT_GOOGLE_CLIENT_ID</code> and <code>VISICONVERT_GOOGLE_CLIENT_SECRET</code> in the server’s private configuration, with its existing encryption key. See the Google connection guide in the installation documentation.</p></details>
    </> : <>
      <p><strong>{status.connected ? "Google sign-in complete" : status.reconnect_required ? "Reconnect Google after the application configuration change" : "Google not connected"}</strong></p>
      <div className="google-actions"><button className="primary" disabled={busy || pending} onClick={connect}>{status.connected ? "Use a different Google login" : "Sign in with Google"}</button>
        {(status.connected || status.revision > 0 || pending) && <button className="secondary" disabled={busy} onClick={() => void run(async () => { await api("/connections/google", { revision: status.revision }, "DELETE"); cleanup.current(); setActions(null); setAccounts([]); setLogins([]); setLogin(""); setNotice("Google disconnected locally. You can also remove VisiConvert access in your Google Account permissions."); })}>Disconnect Google</button>}
        {status.connected && <button className="secondary" disabled={busy} onClick={() => void run(browse)}>Refresh Google Ads accounts</button>}</div>
      {status.connected && !accountsLoaded && !error && <p role="status">Loading your Google Ads accounts…</p>}
      {status.connected && accountsLoaded && logins.length === 0 && <p>No Google Ads accounts are accessible with this Google login. Use a Google user who has access to the advertising account, then sign in again.</p>}
      {status.connected && logins.length > 0 && <div className="site-form google-account-setup">
        <h3>Choose your advertising account</h3>
        {logins.length > 1 && <div className="form-field"><label htmlFor="google-login">Access through a Google Ads account</label><p className="muted" id="google-login-help">Choose your Ads account or manager account to see the advertising accounts it can access. These are Google Ads IDs, not email addresses.</p><select id="google-login" aria-describedby="google-login-help" disabled={busy} value={login} onChange={e => void run(() => chooseLogin(e.target.value))}><option value="">Choose an access account</option>{logins.map(id => <option key={id} value={id}>{accounts.find(a => a.id === id)?.name || "Google Ads account"} · {accountId(id)}</option>)}</select></div>}
        {login && <>
          <div className="form-field"><label htmlFor="google-customer">Advertising account</label><p className="muted" id="google-customer-help">Choose by name and check the ID against Google Ads. The name may be your legal business name.</p><select id="google-customer" aria-describedby="google-customer-help" disabled={busy} value={customer} onChange={e => void run(() => chooseCustomer(e.target.value))}><option value="">Choose an advertising account</option>{accounts.map(c => <option value={c.id} key={c.id}>{c.name || "Unnamed account"} · {accountId(c.id)}{c.manager ? " · Manager" : ""}{c.status !== "ENABLED" ? ` · ${c.status}` : ""}</option>)}{customer && !accounts.some(a => a.id === customer) && <option value={customer}>{actions?.account.name || "Entered account"} · {accountId(customer)}</option>}</select></div>
          {!busy && accounts.length === 0 && <p>No advertising accounts were returned through this access account. Try another access account or enter a known account ID below.</p>}
          {moreAccounts && <p>Showing the first 200 accounts. If yours is missing, enter its account ID below.</p>}
          <details><summary>Can’t find your account? Enter its ID</summary><div className="site-form"><div className="form-field"><label htmlFor="google-manual-customer">Advertising account ID</label><input id="google-manual-customer" disabled={busy} inputMode="numeric" maxLength={12} placeholder="123-456-7890" value={manualCustomer} onChange={e => setManualCustomer(e.target.value.replace(/[^0-9]/g, "").slice(0, 10))} /></div><div><button className="secondary" disabled={busy || !/^\d{10}$/.test(manualCustomer)} onClick={() => void run(() => chooseCustomer(manualCustomer))}>Find advertising account</button></div></div></details>
          {customer && !actions && <p>{busy ? "Loading conversion actions…" : "Conversion actions could not be loaded."}</p>}
          {customer && <div><button className="secondary" disabled={busy} onClick={() => void run(() => chooseCustomer(customer))}>Refresh conversion actions</button></div>}
        </>}
        {actions && <>
          <h3>Choose the outcomes to track</h3>
          <p>Selected account: <strong>{actions.account.name || accountId(customer)}</strong> · {accountId(customer)}. Create actions for the outcomes you want to measure. VisiConvert will reuse actions it has already created for this workspace and account.</p>
          {actions.account.conversion_owner && actions.account.conversion_owner !== customer && <p className="notice">Conversion tracking is owned by {accountId(actions.account.conversion_owner)}. Choose that account above, or enter its ID, to select its actions.</p>}
          <div className="google-create-actions">
            <label className="checkbox-row"><input type="checkbox" checked={createQualified} disabled={busy} onChange={e => setCreateQualified(e.target.checked)} />Qualified leads</label>
            <p className="muted">Counts one qualified lead per ad click, with no monetary value.</p>
            <label className="checkbox-row"><input type="checkbox" checked={createBooking} disabled={busy} onChange={e => setCreateBooking(e.target.checked)} />Bookings</label>
            <p className="muted">Counts each booking and uses its recorded value and currency.</p>
            <p>New actions use a 90-day click window and start as <strong>secondary (reporting only)</strong>. After verifying delivery, choose your bidding outcome in Google Ads. Existing actions keep their current bidding setting.</p>
            <p className="muted">Creates VisiConvert actions in Google Ads and saves your selection here. Sending conversions is a separate step.</p>
            {settings.version > 0 && <p className="notice">This replaces your saved destination mappings with the selected VisiConvert actions. Unselected outcomes will be unmapped. Existing Google actions are not deleted.</p>}
            <button className="primary" disabled={busy || (!createQualified && !createBooking) || actions.account.conversion_owner !== customer} onClick={() => void run(async () => {
              const r = await api<{ qualified_action_id: string; booking_action_id: string }>("/connections/google/actions/create", { revision: status.revision, settings_version: settings.version, login, customer, qualified: createQualified, booking: createBooking });
              await onChange();
              await chooseCustomer(customer);
              setQualified(r.qualified_action_id); setBooking(r.booking_action_id);
              setNotice("Conversion actions created or reused and saved. Check the saved connection below. Delivery remains unchanged.");
            })}>Create conversion actions</button>
          </div>
          <details className="google-existing-actions"><summary>Use existing actions (advanced)</summary><div className="site-form">
            <p>Select an enabled import action for each outcome, leaving others unmapped. Use separate actions for leads and bookings.</p>
          {actions.has_more && <p>Showing the first 200 actions. Advanced destination settings below support other action IDs.</p>}
          {([["google-qualified", "Qualification conversion action", qualified, setQualified], ["google-booking", "Booking conversion action", booking, setBooking]] as const).map(([id, title, value, setter]) => <div className="form-field" key={id}><label htmlFor={id}>{title}</label><select id={id} disabled={busy} value={value} onChange={e => setter(e.target.value)}><option value="">Not mapped</option>{value && !actions.actions.some(a => a.id === value) && <option value={value} disabled>Saved action {value} · not in this list</option>}{actions.actions.map(a => <option key={a.id} value={a.id} disabled={!a.eligible}>{a.name} · {a.id}{a.eligible ? (a.primary ? " · primary" : " · secondary") : ` · ${a.kind} / ${a.status} / owner ${a.owner}`}</option>)}</select></div>)}
          {!actions.actions.some(a => a.eligible) && <p>No compatible existing actions were found. Use Create conversion actions above.</p>}
          {!!qualified && qualified === booking && <p className="error">Choose separate actions for qualification and bookings to avoid counting both as the same outcome.</p>}
          <div><button className="primary" disabled={busy || !validSelection || (!qualified && !booking) || (!!qualified && qualified === booking)} onClick={() => void run(async () => { await api("/conversions/settings", { ...settings, customer_id: customer, qualified_action_id: qualified, booking_action_id: booking }, "PUT"); await onChange(); setNotice("Conversion actions saved. Check the saved connection below to finish setup. Delivery remains unchanged."); })}>Save conversion actions</button></div>
          </div></details>
        </>}
      </div>}
      {status.connected && settings.version > 0 && <div className="google-actions"><button className="secondary" disabled={busy || !login || !actions || !matchesSaved} onClick={() => void run(async () => { const r = await api<Check>("/connections/google/validate", { revision: status.revision, settings_version: settings.version, login }); setNotice(r.message); })}>Check saved connection</button><p className="muted">{matchesSaved && login && actions ? "Check account access and your saved conversion actions. This does not send conversions." : "Choose the saved advertising account and save any changes before checking the connection."}</p></div>}
    </>}
    {validation && matchesSaved && <div className={validation.valid ? "notice" : "error"}><strong>{validation.valid ? "Destination checks passed" : "Destination needs attention"}</strong><p>{validation.message}</p>{status?.checked_at && <small>Checked {new Date(status.checked_at).toLocaleString()}</small>}</div>}
  </section>;
}
