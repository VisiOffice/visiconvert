import { ManagedUsage } from "./ManagedUsage";
import { McpAuthorization, McpConnections } from "./McpAuthorization";
import { useCallback, useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import App, { Brand } from "./App";
import { ArrowRight, Layers3, LogOut, Plus, ShieldCheck } from "./icons";
import { cloudRequest, CloudError, engineAuth, navigate, getAccountLinkToken, clearAccountLinkToken, type Billing, type CloudSession, type Workspace } from "./cloud";

const message = (error: unknown) => error instanceof Error ? error.message : "Something went wrong. Please try again.";
const currentRoute = () => window.location.pathname + window.location.search;
const publicRoute = (path: string) => /^\/(session\/new|registration\/new|passwords\/new|password_reset\/edit|email_verification\/edit)$/.test(path);
const uuid = "[0-9a-fA-F-]{36}";
type Request = <T>(path: string, body?: unknown, method?: string) => Promise<T>;

function Link({ to, children, className }: { to: string; children: ReactNode; className?: string }) {
  return <a href={to} className={className} onClick={event => {
    if (!event.ctrlKey && !event.metaKey && !event.shiftKey && !event.altKey && event.button === 0) {
      event.preventDefault(); navigate(to);
    }
  }}>{children}</a>;
}

export default function HostedApp() {
  const [route, setRoute] = useState(currentRoute);
  const path = route.split("?")[0];
  const [session, setSession] = useState<CloudSession | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [engine, setEngine] = useState<{ token: string; workspace: string; user: string } | null>(null);
  const [opening, setOpening] = useState(false);
  const openingTask = useRef<{ key: string; promise: Promise<{ token: string }> } | null>(null);
  useEffect(() => {
    const update = () => { setRoute(currentRoute()); setError(""); };
    window.addEventListener("popstate", update);
    return () => window.removeEventListener("popstate", update);
  }, []);
  useEffect(() => {
    let active = true;
    cloudRequest<CloudSession>("/session").then(value => { if (active) setSession(value); })
      .catch(error => { if (active) setError(message(error)); });
    return () => { active = false; };
  }, []);
  const request: Request = useCallback(async <T,>(url: string, body?: unknown, method?: string) => {
    try { return await cloudRequest<T>(url, session?.csrf_token, body, method); }
    catch (error) {
      if (error instanceof CloudError && error.status === 401 && url !== "/session") {
        setEngine(null); openingTask.current = null;
        const fresh = await cloudRequest<CloudSession>("/session"); setSession(fresh);
      }
      throw error;
    }
  }, [session?.csrf_token]);
  const workspaceId = path.match(new RegExp(`^/app/(${uuid})$`))?.[1];
  useEffect(() => {
    if (!workspaceId || !session?.user || (engine?.workspace === workspaceId && engine.user === session.user.id)) return;
    let active = true;
    setOpening(true);
    const key = `${session.user.id}:${workspaceId}`;
    const user = session.user.id;
    if (openingTask.current?.key !== key) {
      const promise = (async () => {
        const prepared = await engineAuth<{ state: string }>("prepare", { workspace_id: workspaceId });
        const launched = await request<{ code: string }>(`/workspaces/${workspaceId}/launch`, { state: prepared.state });
        return engineAuth<{ token: string }>("exchange", { code: launched.code });
      })();
      openingTask.current = { key, promise };
    }
    openingTask.current.promise.then(result => {
      if (active) { setEngine({ token: result.token, workspace: workspaceId, user }); setError(""); }
    }).catch(error => { if (active) setError(message(error)); })
      .finally(() => { if (active) setOpening(false); });
    return () => { active = false; };
  }, [workspaceId, session?.user?.id, engine?.workspace, request]);

  const revokeEngine = useCallback(async () => {
    if (engine) {
      const response = await fetch("/api/session", { method: "DELETE", headers: { Authorization: `Bearer ${engine.token}` } });
      if (!response.ok && response.status !== 401) throw new Error("Could not close your workspace session. Please try again.");
    }
    setEngine(null); openingTask.current = null;
  }, [engine]);
  const signOut = useCallback(async () => {
    try {
      await revokeEngine();
      setSession(await request<CloudSession>("/session", undefined, "DELETE"));
      setNotice(""); navigate("/session/new", true);
    } catch (error) { setError(message(error)); throw error; }
  }, [request, revokeEngine]);
  const manageAccount = useCallback(() => navigate("/workspaces"), []);
  const sessionExpired = useCallback(() => {
    setEngine(null); openingTask.current = null;
    setNotice("Your workspace session expired. Open the workspace again to continue.");
    navigate("/workspaces");
  }, []);

  if (!session) return <main className="cloud-loading"><Brand /><p role="status">{error || "Loading your account…"}</p>{error && <button className="primary" onClick={() => window.location.reload()}>Try again</button>}</main>;
  if (engine && workspaceId === engine.workspace && session.user?.id === engine.user) return <App key={engine.token}
    hosted={{ token: engine.token, onManage: manageAccount, onSignOut: signOut, onExpired: sessionExpired }} />;
  if (!session.user || publicRoute(path)) return <AccountForm key={route} path={path} session={session} request={request} notice={notice}
    done={(value, text) => {
      if (value) setSession(value);
      setNotice(text || "");
      if (value?.user) navigate(value.user.email_verified ? (path === "/mcp/authorize" ? route : workspaceId ? path : "/workspaces") : "/email_verification", true);
      else navigate("/session/new", true);
    }} />;

  const detail = path.match(new RegExp(`^/workspaces/(${uuid})(/billing|/authorize)?$`));
  return <div className="app cloud-app">
    <aside className="sidebar"><Brand /><div className="workspace"><span className="workspace-avatar">A</span><div><strong>Your account</strong><small>{session.user.email}</small></div></div>
      <span className="nav-label">ACCOUNT</span><nav><Link className={`cloud-nav ${path !== "/connected-agents" && path !== "/mcp/authorize" ? "active" : ""}`} to="/workspaces"><Layers3 size={18} />Workspaces</Link><Link className={`cloud-nav ${path === "/connected-agents" || path === "/mcp/authorize" ? "active" : ""}`} to="/connected-agents"><ShieldCheck size={18} />Connected agents</Link></nav>
      <div className="sidebar-bottom"><div className="pilot-note"><ShieldCheck size={19} /><strong>Your business, connected.</strong><p>Manage your workspaces and billing in one place.</p></div><button className="signout" onClick={() => void signOut().catch(() => {})}><LogOut size={16} />Sign out</button></div>
    </aside>
    <main className="main"><header className="topbar"><span>VisiConvert · Account</span><Link to="/workspaces/new">New workspace</Link></header>
      <div className="content cloud-content">{notice && <p role="status" className="notice">{notice}</p>}{error && <p role="alert" className="error">{error}</p>}
        {!session.user.email_verified && path !== "/email_verification" && <p className="notice">Verify your email before creating a workspace. <Link to="/email_verification">Check verification</Link></p>}
        {path === "/mcp/authorize" ? <McpAuthorization key={route} request={request} verified={session.user.email_verified} />
          : path === "/connected-agents" ? <McpConnections request={request} />
          : workspaceId ? <section className="card cloud-panel"><h1>Opening your workspace</h1><p role="status">{opening ? "Signing you in securely…" : "The workspace could not be opened."}</p><Link className="primary" to={`/workspaces/${workspaceId}`}>Back to workspace</Link></section>
          : path === "/email_verification" ? <Verification request={request} session={session} refresh={async () => { const value = await request<CloudSession>("/session"); setSession(value); if (value.user?.email_verified) navigate("/workspaces/new"); }} />
          : path === "/workspaces/new" ? <NewWorkspace request={request} verified={session.user.email_verified} />
          : detail ? <WorkspaceDetail key={route} id={detail[1]} billing={detail[2] === "/billing"} authorize={detail[2] === "/authorize"} request={request} beforeOpen={revokeEngine} />
          : path === "/" || path === "/workspaces" ? <Workspaces request={request} />
          : <section className="card cloud-panel"><h1>Page not found</h1><Link to="/workspaces">Go to your workspaces</Link></section>}
      </div>
    </main>
  </div>;
}

function AccountForm({ path, session, request, done, notice }: { path: string; session: CloudSession; request: Request; done: (value?: CloudSession, text?: string) => void; notice: string }) {
  const signup = path === "/registration/new";
  const reset = path === "/password_reset/edit";
  const forgot = path === "/passwords/new";
  const verify = path === "/email_verification/edit";
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); setBusy(true); setError("");
    const form = Object.fromEntries(new FormData(event.currentTarget));
    const token = getAccountLinkToken();
    try {
      if (signup) done(await request<CloudSession>("/registration", { user: form }));
      else if (verify) { const value = await request<CloudSession>("/email_verification", { token }, "PATCH"); clearAccountLinkToken(); done(value, "Your email is verified."); }
      else if (reset || forgot) {
        const result = await request<{ message: string }>(reset ? "/password_reset" : "/passwords", reset ? { ...form, token } : form, reset ? "PATCH" : "POST");
        if (reset) clearAccountLinkToken();
        done(undefined, result.message);
      } else done(await request<CloudSession>("/session", form));
    } catch (error) { setError(message(error)); }
    finally { setBusy(false); }
  }
  const title = signup ? "Create your account." : verify ? "Verify your email." : reset ? "Choose a new password." : forgot ? "Reset your password." : "Welcome back.";
  return <main className="login"><section className="login-story"><Brand /><div><span className="eyebrow">FROM FIRST CALL TO BOOKED REVENUE</span><h1>Know what<br />brings them in.</h1><p>One clear view of your inquiries, marketing sources, and the business they become.</p></div><small><a href="https://visiconvert.com">About VisiConvert</a> · <a href="https://docs.visiconvert.com">Documentation</a></small></section>
    <section className="login-form"><span className="eyebrow">YOUR VISICONVERT ACCOUNT</span><h2>{title}</h2>
      {notice && <p className="notice" role="status">{notice}</p>}
      {signup && !session.signups_enabled ? <><p>Registration is not open yet. Existing customers can still sign in.</p><Link className="primary" to="/session/new">Sign in <ArrowRight size={16} /></Link></> :
        <form onSubmit={submit}>
          {verify ? <p>Confirm this email address to finish setting up your account.</p> : <>
            {!reset && <label>Email<input name="email_address" type="email" autoComplete="email" required maxLength={254} /></label>}
            {!forgot && <label>Password<input name="password" type="password" autoComplete={signup || reset ? "new-password" : "current-password"} required minLength={signup || reset ? 12 : undefined} maxLength={72} /></label>}
            {(signup || reset) && <><label>Confirm password<input name="password_confirmation" type="password" autoComplete="new-password" required minLength={12} maxLength={72} /></label><p className="small muted">Use at least 12 characters.</p></>}
          </>}
          {(verify || reset) && !getAccountLinkToken() && <p role="alert" className="error">This link is missing its token. Request another email.</p>}
          <button className="primary" disabled={busy || ((verify || reset) && !getAccountLinkToken())}>{busy ? "Please wait…" : signup ? "Create account" : verify ? "Verify email address" : reset ? "Save new password" : forgot ? "Send reset instructions" : "Sign in"}</button>
        </form>}
      {error && <p role="alert" className="error">{error}</p>}
      <div className="cloud-form-links">{!signup && !verify && !reset && !forgot && <><Link to="/passwords/new">Forgot password?</Link>{session.signups_enabled && <Link to="/registration/new">Create account</Link>}</>}{(signup || reset || forgot || verify) && <Link to="/session/new">Back to sign in</Link>}{session.user && <Link to="/workspaces">Back to your workspaces</Link>}</div>
    </section></main>;
}

function Verification({ request, session, refresh }: { request: Request; session: CloudSession; refresh: () => Promise<void> }) {
  const [status, setStatus] = useState(""); const [busy, setBusy] = useState(false);
  async function resend() { setBusy(true); try { setStatus((await request<{ message: string }>("/email_verification", {})).message); } catch (error) { setStatus(message(error)); } finally { setBusy(false); } }
  return <section className="card cloud-panel"><h1>Check your inbox.</h1><p>We sent a verification link to <strong>{session.user?.email}</strong>. Open it to finish setting up your account.</p><div className="cloud-actions"><button className="primary" disabled={busy} onClick={() => void refresh().catch(error => setStatus(message(error)))}>I’ve verified my email</button><button className="secondary" disabled={busy} onClick={() => void resend()}>Resend verification email</button></div>{status && <p role="status">{status}</p>}</section>;
}

function Workspaces({ request }: { request: Request }) {
  const [items, setItems] = useState<Workspace[] | null>(null); const [error, setError] = useState("");
  useEffect(() => { let active = true; request<{ workspaces: Workspace[] }>("/workspaces").then(result => { if (active) setItems(result.workspaces); }).catch(error => { if (active) setError(message(error)); }); return () => { active = false; }; }, [request]);
  return <><div className="page-title"><div><span className="eyebrow">YOUR BUSINESSES</span><h1>Your workspaces</h1><p>Open a business workspace or manage its subscription.</p></div><Link className="primary" to="/workspaces/new"><Plus size={16} />New workspace</Link></div>{error && <p role="alert" className="error">{error}</p>}
    {!items ? <p role="status">Loading workspaces…</p> : items.length === 0 ? <section className="card cloud-panel"><h2>Create your first workspace.</h2><p>Keep each business’s inquiries, numbers, and reporting together.</p><Link className="primary" to="/workspaces/new">Create workspace <ArrowRight size={16} /></Link></section> : <div className="cloud-workspaces">{items.map(item => <section className="card cloud-panel" key={item.id}><span className="chip">{!item.paid_access ? "Billing needed" : item.status === "ready" ? "Ready" : item.status === "pending" ? "Setting up" : "Setup needs attention"}</span><h2>{item.name}</h2><Link className="primary" to={`/workspaces/${item.id}`}>View workspace <ArrowRight size={16} /></Link>{item.can_manage_billing && <Link to={`/workspaces/${item.id}/billing`}>Billing</Link>}</section>)}</div>}</>;
}

function NewWorkspace({ request, verified }: { request: Request; verified: boolean }) {
  const [busy, setBusy] = useState(false); const [error, setError] = useState("");
  async function submit(event: FormEvent<HTMLFormElement>) { event.preventDefault(); setBusy(true); setError(""); try { const workspace = await request<Workspace>("/workspaces", { workspace: { name: new FormData(event.currentTarget).get("name") } }); navigate(`/workspaces/${workspace.id}`); } catch (error) { setError(message(error)); } finally { setBusy(false); } }
  return <section className="card cloud-panel"><h1>Create a workspace.</h1><p>Use your business name. Each workspace keeps its data separate.</p>{verified ? <form onSubmit={submit}><label>Workspace name<input name="name" required maxLength={100} /></label><button className="primary" disabled={busy}>{busy ? "Creating…" : "Create workspace"}</button></form> : <Link className="primary" to="/email_verification">Verify your email to continue</Link>}{error && <p role="alert" className="error">{error}</p>}<Link to="/workspaces">All workspaces</Link></section>;
}

function WorkspaceDetail({ id, billing, authorize, request, beforeOpen }: { id: string; billing: boolean; authorize: boolean; request: Request; beforeOpen: () => Promise<void> }) {
  const [workspace, setWorkspace] = useState<Workspace | null>(null); const [plan, setPlan] = useState<Billing | null>(null);
  const [error, setError] = useState(""); const [notice, setNotice] = useState(""); const [busy, setBusy] = useState(false);
  const reload = useCallback(async () => { const value = await request<Workspace>(`/workspaces/${id}`); setWorkspace(value); if (billing) setPlan(await request<Billing>(`/workspaces/${id}/billing`)); }, [id, billing, request]);
  useEffect(() => { void reload().catch(error => setError(message(error))); }, [reload]);
  useEffect(() => { if (!workspace || workspace.status !== "pending" || !workspace.paid_access) return; const timer = window.setInterval(() => { void reload().catch(error => setError(message(error))); }, 3000); return () => clearInterval(timer); }, [workspace?.status, workspace?.paid_access, reload]);
  async function action(name: "checkout" | "portal" | "refresh" | "retry_provisioning" | "open") {
    setBusy(true); setError("");
    try {
      if (name === "open") { await beforeOpen(); navigate(`/app/${id}`); return; }
      if (name === "retry_provisioning") { await request(`/workspaces/${id}/retry_provisioning`, {}); await reload(); return; }
      const result = await request<{ url?: string; message?: string }>(`/workspaces/${id}/billing/${name}`, {});
      if (result.url) {
        const destination = new URL(result.url); const expected = name === "checkout" ? "checkout.stripe.com" : "billing.stripe.com";
        if (destination.protocol !== "https:" || destination.hostname !== expected || destination.username || destination.password) throw new Error("Unexpected billing destination.");
        window.location.assign(destination.href);
      } else { setNotice(result.message || "Status updated."); await reload(); }
    } catch (error) { setError(message(error)); }
    finally { setBusy(false); }
  }
  if (!workspace) return <section className="card cloud-panel"><h1>Workspace</h1><p role={error ? "alert" : "status"}>{error || "Loading workspace…"}</p><Link to="/workspaces">All workspaces</Link></section>;
  return <section className="card cloud-panel"><span className="eyebrow">{billing ? "WORKSPACE BILLING" : "YOUR WORKSPACE"}</span><h1>{workspace.name}</h1>{error && <p className="error" role="alert">{error}</p>}{notice && <p className="notice" role="status">{notice}</p>}
    {billing ? !plan ? <p>Loading billing…</p> : <>
      {plan.mode === "test" && <p className="notice">Test billing · no real payments</p>}
      {!plan.required ? plan.carrier_migration ? <><h2>Move to paid managed service</h2><p>Your existing access and call routing stay available during the switch.</p><p>{plan.can_checkout ? "Subscribe for $27.50/month per workspace, including $27.50 in monthly usage credit. Local numbers cost $2/month, prorated, and connected forwarding costs 4¢ per rounded minute." : "We’re preparing or verifying your subscription before activating managed service."}</p><p>Managed usage starts at activation. Earlier test calls and number rental won’t be billed.</p></> : <p>This workspace has pilot access. A subscription is not required.</p> : <><h2>{new Intl.NumberFormat("en-US", { style: "currency", currency: plan.currency }).format(plan.amount_minor / 100)} / month</h2><p>One business workspace, billed monthly. Phone numbers and carrier usage are separate.</p><p>{plan.paid_access ? "Your subscription is active." : plan.status === "past_due" ? "Your payment needs attention. Update your payment method to restore access." : plan.status === "review_required" ? "Your billing setup needs review. Please contact support." : `Subscription status: ${plan.status.replaceAll("_", " ")}.`}</p>{plan.cancellation_scheduled && plan.access_until && <p>Cancellation is scheduled. Access continues until {new Date(plan.access_until).toLocaleDateString()}.</p>}</>}
      <div className="cloud-actions">{plan.can_checkout && <button className="primary" disabled={busy} onClick={() => void action("checkout")}>Subscribe for $27.50/month</button>}{plan.can_manage && <><button className="primary" disabled={busy} onClick={() => void action("portal")}>Manage billing</button><button className="secondary" disabled={busy} onClick={() => void action("refresh")}>Check payment status</button></>}<button className="secondary" disabled={busy} onClick={() => void reload().catch(error => setError(message(error)))}>Refresh status</button></div><Link to={`/workspaces/${id}`}>Back to workspace</Link>
      <ManagedUsage workspaceId={id} request={request} />
    </> : <>
      {!workspace.paid_access ? <p>Activate your subscription to open this workspace.</p> : workspace.status === "ready" ? <><p>Your workspace is ready. Review inquiries and see what brings in business.</p><button className="primary" disabled={busy} onClick={() => void action("open")}>{authorize ? "Continue to workspace" : "Open workspace"}<ArrowRight size={16} /></button></> : workspace.status === "failed" ? <><p>We couldn’t finish setting up this workspace. Your account is safe; retry setup to continue.</p><button className="primary" disabled={busy} onClick={() => void action("retry_provisioning")}>Retry setup</button></> : <p role="status">We’re preparing your workspace. This usually takes a moment.</p>}
      {workspace.can_manage_billing && <Link to={`/workspaces/${id}/billing`}>Billing</Link>}<button className="secondary" disabled={busy} onClick={() => void reload().catch(error => setError(message(error)))}>Check status</button>
    </>}<Link to="/workspaces">All workspaces</Link></section>;
}
