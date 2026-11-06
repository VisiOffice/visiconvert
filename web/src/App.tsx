import { SignalWireConnection } from "./SignalWireConnection";
import type { SignalWireConnection as SignalWireStatus } from "./api";
import { useEffect, useMemo, useState, type FormEvent } from "react";
import { InquiryGrouping } from "./InquiryGrouping";
import { Setup } from "./Setup";
import { ApiKeys } from "./ApiKeys";
import { Webhooks } from "./Webhooks";
import { Conversions } from "./Conversions";
import { Sites } from "./Sites";
import { TwilioConnection } from "./TwilioConnection";
import { Logs } from "./Logs";
import { parse, go as visit, type Page, type Route } from "./routes";
import {
  ArrowDownLeft,
  ArrowRight,
  BarChart3,
  Check,
  ChevronRight,
  Filter,
  Layers3,
  Link2,
  Loader2,
  LogOut,
  Phone,
  Plus,
  Search,
  ShieldCheck,
  Sparkles,
  Users,
  Activity,
  ScrollText,
  X,
} from "./icons";
import {
  client,
  money,
  label,
  type Inquiry as Lead,
  type TrackingNumber,
  type Summary,
  type Bootstrap,
  type Client,
  type Outcome,
  type TwilioConnection as Connection,
  type TwilioInventory,
} from "./api";
import initWasm, {
  preview_attribution,
} from "./generated/attribution/convert_wasm";

type NavPage = { name: Page; icon: typeof BarChart3 };
// Button labels are part of the browser tests (engine and Cloud); keep them exact.
const groups: { label: string; pages: NavPage[] }[] = [
  { label: "Workspace", pages: [
    { name: "Overview", icon: BarChart3 },
    { name: "Lead inbox", icon: Users },
    { name: "Tracking numbers", icon: Phone },
    { name: "Sites", icon: Layers3 },
  ] },
  { label: "Advertising", pages: [
    { name: "Conversions", icon: Check },
    { name: "Connections", icon: Link2 },
  ] },
  { label: "Developers", pages: [
    { name: "API keys", icon: ShieldCheck },
    { name: "Webhooks", icon: Activity },
    { name: "Logs", icon: ScrollText },
  ] },
];
const pageMeta: Record<Page, { group: string; eyebrow?: string; title?: string; text: string }> = {
  Setup: { group: "Get started", text: "Connect your agent, choose how calls are tracked, and check what VisiConvert has observed." },
  Overview: { group: "Workspace", eyebrow: "ATTRIBUTION, WITH CONTEXT", title: "See what’s working.", text: "Connect every inquiry to the source that started it." },
  "Lead inbox": { group: "Workspace", text: "Review inquiries, qualify opportunities, and record bookings." },
  "Tracking numbers": { group: "Workspace", text: "Give each campaign a number. Keep your existing phone system." },
  Sites: { group: "Workspace", text: "Connect website visits and successful forms to their source." },
  Conversions: { group: "Advertising", text: "Send pipeline conversions to Google Ads and Microsoft Advertising, and review what was sent." },
  Connections: { group: "Advertising", text: "Connect Twilio or SignalWire and preview how a landing page is attributed." },
  "API keys": { group: "Developers", text: "Give scripts, the CLI and agents scoped access to this workspace." },
  Webhooks: { group: "Developers", text: "Send signed events to your CRM and automations." },
  Logs: { group: "Developers", text: "Everything that happened in this workspace, newest first." },
};
type SetupEvidence = { sites: { last_session_at: string | null }[]; numbers: { last_call_at: string | null }[] };
type HostedShell = { token: string; onManage: () => void; onSignOut: () => Promise<void>; onExpired: () => void };
export default function App({ hosted }: { hosted?: HostedShell }) {
  const [key, setKey] = useState(hosted?.token || "");
  const [setupKey, setSetupKey] = useState(false);
  const [entry, setEntry] = useState("");
  const [route, setRoute] = useState<Route>(() => parse());
  const page = route.page;
  // The address bar names the page (and tab), so reloads, links and Back/Forward work.
  const show = (next: Page, tab: string | null = null) => { const r = { page: next, tab }; visit(r); setRoute(r); };
  useEffect(() => {
    visit(route, true); // Normalize unknown paths without adding history.
    const update = () => setRoute(parse());
    window.addEventListener("popstate", update);
    return () => window.removeEventListener("popstate", update);
  }, []);
  const [boot, setBoot] = useState<Bootstrap | null>(null);
  const [signalwire, setSignalwire] = useState<SignalWireStatus | null>(null);
  const [connection, setConnection] = useState<Connection | null>(null);
  const [leads, setLeads] = useState<Lead[]>([]);
  const [numbers, setNumbers] = useState<TrackingNumber[]>([]);
  const [summary, setSummary] = useState<Summary[]>([]);
  const [evidence, setEvidence] = useState<SetupEvidence | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [selected, setSelected] = useState<Lead | null>(null);
  const [modal, setModal] = useState<"number" | "lead" | null>(null);
  const [businessProfile, setBusinessProfile] = useState(false);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("all");
  const api = useMemo(() => client(key, hosted?.onExpired), [key, hosted?.onExpired]);
  useEffect(() => {
    // On phones the navigation is one scrolling row; keep the current page in view.
    if (window.matchMedia("(max-width: 760px)").matches)
      document.querySelector(".sidebar nav [aria-current='page']")?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [page]);
  useEffect(() => { if (hosted) setKey(hosted.token); }, [hosted?.token]);
  useEffect(() => {
    // Load once per workspace; a renewed token must not reload the page under the user.
    if (hosted) void refresh(client(hosted.token, hosted.onExpired)).catch(error => setError(error instanceof Error ? error.message : "Unable to load workspace"));
  }, []);
  useEffect(() => {
    const code = new URLSearchParams(window.location.hash.slice(1)).get("launch");
    if (!code) return;
    // Remove the one-use credential from browser history before any asynchronous work.
    window.history.replaceState(null, "", window.location.pathname + window.location.search);
    setLoading(true);
    setError("");
    void (async () => {
      try {
        const response = await fetch("/auth/exchange", {
          method: "POST", credentials: "same-origin",
          headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code }),
        });
        const result = await response.json();
        if (!response.ok) throw new Error(result.error || "Unable to open this workspace");
        await refresh(client(result.token));
        setKey(result.token);
      } catch (error) {
        setError(error instanceof Error ? error.message : "Unable to open this workspace");
      } finally { setLoading(false); }
    })();
  }, []);

  async function refresh(request = api) {
    const [b, l, n, s, c, sw, e] = await Promise.all([
      request<Bootstrap>("/bootstrap"),
      request<Lead[]>("/inquiries"),
      request<TrackingNumber[]>("/numbers"),
      request<Summary[]>("/summary"),
      request<Connection>("/connections/twilio"),
      request<SignalWireStatus>("/connections/signalwire"),
      // Setup progress is a sidebar hint; never block the workspace on it.
      request<SetupEvidence>("/setup").catch(() => null),
    ]);
    setEvidence(e);
    setBoot(b);
    setConnection(c);
    setSignalwire(sw);
    setLeads(l);
    setNumbers(n);
    setSummary(s);
    setSelected((current) =>
      current ? l.find((item) => item.id === current.id) || null : null,
    );
  }
  async function login(e: FormEvent) {
    e.preventDefault();
    setError("");
    setLoading(true);
    try {
      await refresh(client(entry));
      setKey(entry);
      setEntry("");
    } catch (e) {
      setError(String(e instanceof Error ? e.message : e));
    } finally {
      setLoading(false);
    }
  }
  const totals = summary.reduce(
    (a, s) => ({
      leads: a.leads + s.leads,
      qualified: a.qualified + s.qualified,
      bookings: a.bookings + s.bookings,
      booked_minor: a.booked_minor + s.booked_minor,
    }),
    { leads: 0, qualified: 0, bookings: 0, booked_minor: 0 },
  );
  const filtered = leads.filter(
    (l) =>
      (filter === "all" || l.qualification === filter) &&
      l.search_text
        .toLowerCase()
        .includes(query.toLowerCase()),
  );
  // Setup is finished once a real (non-demo) inquiry exists, from any source including the API.
  const setupComplete = leads.some(l => !l.is_demo);
  // A site or number is registered: nothing left to do but wait for the first lead.
  const setupReady = evidence ? evidence.sites.length > 0 || evidence.numbers.length > 0 : numbers.length > 0;
  const go = (next: Page) => { show(next); setSetupKey(false); setError(""); };
  const navButton = (p: NavPage) => (
    <button key={p.name} className={page === p.name ? "active" : ""} aria-current={page === p.name ? "page" : undefined} onClick={() => go(p.name)}>
      <p.icon size={17} />
      {p.name}
      {p.name === "Lead inbox" && <span className="count">{totals.leads}</span>}
    </button>
  );
  const meta = pageMeta[page];
  if (!key)
    return (
      <main className="login">
        <section className="login-story">
          <Brand />
          <div>
            <span className="eyebrow">FROM FIRST CALL TO BOOKED REVENUE</span>
            <h1>
              Know what
              <br />
              brings them in.
            </h1>
            <p>
              One clear view of your inquiries, marketing sources, and the
              business they become.
            </p>
            <div className="flow">
              <span>
                <Phone size={16} /> Inquiry
              </span>
              <ArrowRight size={16} />
              <span>
                <Check size={16} /> Qualified
              </span>
              <ArrowRight size={16} />
              <span>Booked</span>
            </div>
          </div>
          <small>
            Open source. Your numbers. Your data. ·{" "}
            <a href="https://github.com/VisiOffice/visiconvert">Source</a> ·{" "}
            <a href="/LICENSE.txt">AGPL-3.0-only</a>
          </small>
        </section>
        <section className="login-form">
          <div className="pilot-label">EARLY ACCESS</div>
          <h2>Open your workspace</h2>
          <p>
            Use the workspace key provided by your administrator. It stays in
            memory for this session.
          </p>
          <form onSubmit={login}>
            <label>
              Workspace API key
              <input
                type="password"
                autoComplete="off"
                value={entry}
                onChange={(e) => setEntry(e.target.value)}
                required
                minLength={32}
                autoFocus
              />
            </label>
            <button className="primary" disabled={loading}>
              {loading ? (
                <Loader2 className="spin" size={18} />
              ) : (
                <>
                  Open workspace <ArrowRight size={16} />
                </>
              )}
            </button>
          </form>
          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}
          <p className="muted small">
            Self-hosting? Find your workspace key in your installation’s config.env.
          </p>
        </section>
      </main>
    );
  return (
    <div className="app">
      <aside className="sidebar">
        <Brand />
        <div className="workspace">
          <span className="workspace-avatar">W</span>
          <div>
            <strong>{boot?.name || "My workspace"}</strong>
            <small>{boot?.hosted ? "Hosted workspace" : "Self-hosted pilot"}</small>
          </div>
        </div>
        <nav aria-label="Workspace pages">
          {!setupComplete && (
            <>
              <button className={`nav-setup ${setupReady ? "ready" : ""} ${page === "Setup" ? "active" : ""}`} aria-current={page === "Setup" ? "page" : undefined} aria-describedby="setup-progress" onClick={() => go("Setup")}>
                <span className="nav-setup-title"><Sparkles size={17} />Setup</span>
                {setupReady
                  ? <span className="nav-setup-next" aria-hidden="true"><i className="nav-setup-dot" />Ready · waiting for your first lead</span>
                  : <span className="nav-setup-next" aria-hidden="true">Next: add a website or tracking number</span>}
              </button>
              <span id="setup-progress" className="sr-only">{setupReady ? "Ready, waiting for your first lead" : "Next step: add a website or tracking number"}</span>
            </>
          )}
          {groups.map(g => (
            <div className="nav-group" role="group" aria-labelledby={`nav-${g.label}`} key={g.label}>
              <span className="nav-label" id={`nav-${g.label}`}>{g.label}</span>
              {g.pages.map(navButton)}
            </div>
          ))}
          {setupComplete && (
            <div className="nav-group nav-group-setup">
              <button className={page === "Setup" ? "active" : ""} aria-current={page === "Setup" ? "page" : undefined} onClick={() => go("Setup")}>
                <Sparkles size={17} />Setup
              </button>
            </div>
          )}
        </nav>
        <div className="sidebar-bottom">
          {hosted && <button className="signout" onClick={hosted.onManage}><Layers3 size={16} />Workspaces & billing</button>}
          <div className="pilot-note">
            <ShieldCheck size={19} />
            <strong>Your stack, connected.</strong>
            <p>
              {boot?.hosted
                ? "Add inquiries, qualify opportunities, and connect bookings to their source."
                : "Track with Twilio or SignalWire. Answer calls in the phone system you already use."}
            </p>
          </div>
          <button
            className="signout"
            onClick={async () => {
              if (hosted) { try { await hosted.onSignOut(); } catch (error) { setError(error instanceof Error ? error.message : "Unable to sign out"); } return; }
              if (key.startsWith("vc_session_")) {
                try {
                  const response = await fetch("/api/session", { method: "DELETE", headers: { Authorization: `Bearer ${key}` } });
                  if (!response.ok && response.status !== 401) throw new Error("Unable to sign out. Please try again.");
                } catch (error) { setError(error instanceof Error ? error.message : "Unable to sign out"); return; }
              }
              setKey("");
              setBoot(null);
              setConnection(null);
              setSelected(null);
              setModal(null);
              setError("");
            }}
          >
            <LogOut size={16} /> {key.startsWith("vc_session_") ? "Sign out" : "Lock workspace"}
          </button>
        </div>
      </aside>
      <main className="main">
        <header className="topbar">
          <span>
            {meta.group} <ChevronRight size={13} /> <strong>{page}</strong>
          </span>
          <span className="status">
            <i /> {boot?.hosted ? "Hosted pilot" : "Local pilot"}
          </span>
        </header>
        <div className="content">
          <div className="page-title">
            <div>
              <span className="eyebrow">{meta.eyebrow || meta.group.toUpperCase()}</span>
              <h1>{meta.title || page}</h1>
              <p>{meta.text}</p>
            </div>
            {page !== "Setup" && page !== "Sites" && page !== "Conversions" && page !== "API keys" && page !== "Webhooks" && page !== "Logs" && <button
              className="primary"
              disabled={boot?.hosted && !connection?.connected && !signalwire?.connected && page === "Tracking numbers"}
              onClick={() => { setBusinessProfile(false); setModal(page === "Tracking numbers" ? "number" : "lead"); }}
            >
              <Plus size={16} />
              {page === "Tracking numbers"
                ? "Add tracking number"
                : "Add form lead"}
            </button>}
          </div>
          {page === "Tracking numbers" && !connection?.connected && !signalwire?.connected && <p className="notice">Connect Twilio or SignalWire in Connections to receive calls. {boot?.hosted ? "Connect before registering a tracking number." : "Offline number registrations do not route calls."}</p>}
          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}
          {leads.some((l) => l.is_demo) && (
            <div className="demo-banner">
              <Sparkles size={15} />
              <span>
                Sample data is included in this workspace. Demo leads are
                labeled throughout.
              </span>
            </div>
          )}
          {page === "Overview" && !leads.some(l => !l.is_demo) && <section className="card setup-welcome">
            <span className="eyebrow">START HERE</span><h2>Let’s connect your first source.</h2>
            <p>Your agent can help set up websites and call tracking. Follow the same steps yourself if you prefer.</p>
            <button className="primary" onClick={() => show("Setup")}>Set up this workspace</button>
          </section>}
          {page === "Setup" && <Setup hosted={!!hosted} api={api} onBusinessProfile={() => { show("Tracking numbers"); setBusinessProfile(true); if (!boot?.hosted || connection?.connected || signalwire?.connected) setModal("number"); }} onNavigate={p => show(p)} onKey={() => { setSetupKey(true); show("API keys"); }} />}
          {page === "Overview" && (
            <>
              <div className="section-heading">
                <h2>Performance at a glance</h2>
                <span>All time · USD</span>
              </div>
              <div className="metrics">
                {[
                  {
                    title: "Total inquiries",
                    value: totals.leads,
                    sub: "Grouped calls and submitted forms",
                    icon: ArrowDownLeft,
                  },
                  {
                    title: "Qualified leads",
                    value: totals.qualified,
                    sub: totals.leads
                      ? `${Math.round((totals.qualified / totals.leads) * 100)}% of all inquiries`
                      : "Review leads to get started",
                    icon: Users,
                  },
                  {
                    title: "Bookings",
                    value: totals.bookings,
                    sub: "Distinct booked outcomes",
                    icon: Check,
                  },
                  {
                    title: "Booked revenue",
                    value: money(totals.booked_minor),
                    sub: "Recorded bookings · not payments",
                    icon: BarChart3,
                  },
                ].map((m) => (
                  <article className="metric" key={m.title}>
                    <div>
                      {m.title}
                      <m.icon size={18} />
                    </div>
                    <strong>{m.value}</strong>
                    <small>{m.sub}</small>
                  </article>
                ))}
              </div>
              <section className="card sources">
                <div className="card-header">
                  <div>
                    <h2>Where your leads come from</h2>
                    <p>
                      Each inquiry counts once, using its chosen reporting source.
                      Individual interaction sources remain available in its timeline.
                    </p>
                  </div>
                  <span className="chip">All sources</span>
                </div>
                {summary.length ? (
                  <div className="table-scroll">
                    <table>
                      <thead>
                        <tr>
                          <th>Marketing source</th>
                          <th>Inquiries</th>
                          <th>Qualified</th>
                          <th>Bookings</th>
                          <th>Booked revenue</th>
                        </tr>
                      </thead>
                      <tbody>
                        {summary.map((s) => (
                          <tr key={s.source}>
                            <td>
                              <strong>{s.source}</strong>
                              <div className="source-bar">
                                <i
                                  style={{
                                    width: `${(s.leads / Math.max(...summary.map((x) => x.leads))) * 100}%`,
                                  }}
                                />
                              </div>
                            </td>
                            <td>{s.leads}</td>
                            <td>{s.qualified}</td>
                            <td>{s.bookings}</td>
                            <td className="revenue">{money(s.booked_minor)}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                ) : (
                  <Empty
                    title="Your sources will appear here"
                    text="Register a campaign number or add your first form lead."
                  />
                )}
              </section>
            </>
          )}
          {(page === "Overview" || page === "Lead inbox") && (
            <section className="card">
              <div className="card-header">
                <div>
                  <h2>
                    {page === "Overview" ? "Recent inquiries" : "All inquiries"}
                  </h2>
                  <p>
                    {page === "Overview"
                      ? "The latest conversations and opportunities."
                      : "Showing up to the latest 500 inquiries."}
                  </p>
                </div>
                {page === "Overview" && (
                  <button
                    className="text-button"
                    onClick={() => show("Lead inbox")}
                  >
                    View inbox <ArrowRight size={15} />
                  </button>
                )}
              </div>
              {page === "Lead inbox" && (
                <div className="toolbar">
                  <label className="search">
                    <Search size={16} />
                    <input
                      aria-label="Search leads"
                      placeholder="Search names, sources, campaigns…"
                      value={query}
                      onChange={(e) => setQuery(e.target.value)}
                    />
                  </label>
                  <label className="filter">
                    <Filter size={15} />
                    <select
                      aria-label="Qualification filter"
                      value={filter}
                      onChange={(e) => setFilter(e.target.value)}
                    >
                      <option value="all">All qualifications</option>
                      {["unreviewed", "qualified", "unqualified", "spam"].map(
                        (v) => (
                          <option key={v} value={v}>
                            {label(v)}
                          </option>
                        ),
                      )}
                    </select>
                  </label>
                </div>
              )}
              <LeadTable
                leads={page === "Overview" ? leads.slice(0, 5) : filtered}
                select={setSelected}
              />
            </section>
          )}
          {page === "Tracking numbers" && (
            <>
              <section className="card business-profile-guide" aria-label="Google Business Profile setup">
                <h2>Google Business Profile tracking</h2>
                <p>One dedicated static number per listing. No website tracker or pool is needed for calls made directly from the listing.</p>
                <ol><li>Register an owned number with source <strong>Google Business Profile</strong>, a location name, and the line where calls should ring. Confirm the number is available before replacing any existing carrier route.</li><li>Configure its voice and status callbacks in your carrier using Connections. Call it and confirm the business answers and the lead appears in VisiConvert.</li><li>In your Google Business Profile, set the tracking number as the primary phone and keep your business number as an additional phone. Keep the tracking number outside all website pools.</li></ol>
                <p className="muted">VisiConvert does not edit your Google listing. Calls get listing-level attribution, without a visitor click ID. Ads using this listing’s phone number can also reach it, so this source does not prove a call was organic.</p>
                <button className="secondary" disabled={boot?.hosted && !connection?.connected && !signalwire?.connected} onClick={() => { setBusinessProfile(true); setModal("number"); }}>Add Business Profile number</button>
              </section>
              <div className="routing-diagram">
                <span>
                  <Phone size={18} /> Tracking number
                </span>
                <ArrowRight size={19} />
                <span>
                  <Layers3 size={18} /> VisiConvert attribution
                </span>
                <ArrowRight size={19} />
                <span>
                  <Users size={18} /> Your business phone
                </span>
              </div>
              <section className="card">
                <div className="card-header">
                  <div>
                    <h2>Campaign numbers</h2>
                    <p>
                      Register numbers you already own in Twilio or SignalWire. This does not
                      purchase a number.
                    </p>
                  </div>
                  <span className="chip">{numbers.length} registered</span>
                </div>
                {numbers.length ? (
                  <div className="table-scroll">
                    <table>
                      <thead>
                        <tr>
                          <th>Tracking number</th>
                          <th>Source / campaign</th>
                          <th>Forwards to</th>
                        </tr>
                      </thead>
                      <tbody>
                        {numbers.map((n) => (
                          <tr key={n.id}>
                            <td>
                              <strong>{n.label}</strong>
                              <small>{n.phone_number} · {n.provider === "signalwire" ? "SignalWire" : "Twilio"}</small>
                            </td>
                            <td>
                              {n.source}
                              <small>{n.campaign}</small>
                            </td>
                            <td>
                              {n.destination}
                              <small>
                                {n.is_demo
                                  ? "Demo only"
                                  : `Configure webhook in ${n.provider === "signalwire" ? "SignalWire" : "Twilio"}`}
                              </small>
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                ) : (
                  <Empty
                    title="Your first campaign starts here"
                    text="Add an owned Twilio or SignalWire number and the business or call center line that should receive its calls."
                  />
                )}
              </section>
              <p className="footnote">
                Tracking accepts +1 (NANP) numbers. Configure website pools in Sites.
                SMS and number purchasing are not included.
              </p>
            </>
          )}
          {page === "Sites" && <Sites api={api} />}
          {page === "Conversions" && <Conversions api={api} tab={route.tab} onTab={t => show("Conversions", t === "overview" ? null : t)} />}
          {page === "API keys" && <ApiKeys api={api} setup={setupKey} />}
          {page === "Webhooks" && <Webhooks api={api} />}
          {page === "Logs" && <Logs api={api} onOpenLead={async (id) => {
            const detail = await api<{ inquiry: Lead }>(`/inquiries/${id}`);
            setSelected(detail.inquiry);
          }} />}
          {page === "Connections" && connection && <Connections api={api} connection={connection} signalwire={signalwire} refresh={refresh} />}
          <footer>
            VisiConvert <span>Make the connection.</span>
            <a href="https://github.com/VisiOffice/visiconvert">Source</a>
            <a href="/LICENSE.txt">License</a>
            <button
              className="text-button"
              onClick={async () => {
                try {
                  await refresh();
                  setError("");
                } catch (e) {
                  setError((e as Error).message);
                }
              }}
            >
              Refresh data
            </button>
          </footer>
        </div>
      </main>
      {selected && (
        <LeadDetail
          key={selected.id}
          lead={selected}
          api={api}
          refresh={refresh}
          changed={async (id) => {
            await refresh();
            const detail = await api<{ inquiry: Lead }>(`/inquiries/${id}`);
            setSelected(detail.inquiry);
          }}
          close={() => setSelected(null)}
        />
      )}{" "}
      {modal && (
        <CreateModal
          kind={modal}
          businessProfile={businessProfile && modal === "number"}
          api={api}
          connected={connection?.connected || false}
          signalwireConnected={signalwire?.connected || false}
          close={() => setModal(null)}
          done={async () => {
            await refresh();
            setModal(null);
          }}
        />
      )}
    </div>
  );
}
export function Brand() {
  return (
    <div className="brand">
      <span className="brand-icon">
        <img src="/brand/visiconvert-mark.svg" width="25" height="24" alt="" />
      </span>
      <strong>
        visi<span>convert</span>
      </strong>
    </div>
  );
}
function Empty({ title, text }: { title: string; text: string }) {
  return (
    <div className="empty">
      <span className="empty-icon"><Layers3 size={22} /></span>
      <h3>{title}</h3>
      <p>{text}</p>
    </div>
  );
}
function LeadTable({
  leads,
  select,
}: {
  leads: Lead[];
  select: (lead: Lead) => void;
}) {
  return leads.length ? (
    <div className="table-scroll">
      <table>
        <thead>
          <tr>
            <th>Inquiry</th>
            <th>Source</th>
            <th>Qualification</th>
            <th>Booked value</th>
            <th>Received</th>
          </tr>
        </thead>
        <tbody>
          {leads.map((l) => (
            <tr key={l.id}>
              <td>
                <button className="lead-button" onClick={() => select(l)}>
                  <span className={`channel ${l.channel}`}>
                    {l.channel === "call" ? (
                      <Phone size={15} />
                    ) : (
                      <Layers3 size={15} />
                    )}
                  </span>
                  <span>
                    <strong>{l.name}</strong>
                    <small>
                      {l.contact} · {l.interaction_count} {l.interaction_count === 1 ? "interaction" : "interactions"} {l.is_demo && <em>DEMO</em>}
                    </small>
                  </span>
                </button>
              </td>
              <td>
                {l.source}
                <small>{l.campaign || "No campaign supplied"}</small>
              </td>
              <td>
                <span className={`badge ${l.qualification}`}>
                  <i />
                  {label(l.qualification)}
                </span>
              </td>
              <td>{l.booked_minor ? money(l.booked_minor) : "—"}</td>
              <td className="date">
                {new Date(l.created_at).toLocaleDateString(undefined, {
                  month: "short",
                  day: "numeric",
                })}
                <small>
                  {new Date(l.created_at).toLocaleTimeString(undefined, {
                    hour: "numeric",
                    minute: "2-digit",
                  })}
                </small>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  ) : (
    <Empty
      title="No inquiries to show"
      text="Add a form lead, connect a tracking number, or adjust your search."
    />
  );
}
function LeadDetail({
  lead,
  api,
  refresh,
  close,
  changed,
}: {
  lead: Lead;
  api: Client;
  refresh: () => Promise<void>;
  close: () => void;
  changed: (id: string) => Promise<void>;
}) {
  const [qualification, setQualification] = useState(lead.qualification),
    [notes, setNotes] = useState(lead.notes),
    [editVersion, setEditVersion] = useState(lead.version),
    [error, setError] = useState(""),
    [notice, setNotice] = useState(""),
    [busy, setBusy] = useState(false),
    [outcomes, setOutcomes] = useState<Outcome[]>([]);
  const dirty = notes !== lead.notes || qualification !== lead.qualification;
  useEffect(() => { setQualification(lead.qualification); setNotes(lead.notes); setEditVersion(lead.version); }, [lead.id, lead.version, lead.notes, lead.qualification]);
  const [booking, setBooking] = useState(""),
    [amount, setAmount] = useState("");
  useEffect(() => {
    let active = true;
    api<Outcome[]>(`/inquiries/${lead.id}/outcomes`)
      .then(value => { if (active) setOutcomes(value); })
      .catch((e) => { if (active) setError(e.message); });
    return () => { active = false; };
  }, [api, lead.id, lead.version]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [close]);
  async function run(action: () => Promise<void>) {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await action();
      await refresh();
      setNotice("Saved");
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="overlay">
      <section
        className="drawer"
        role="dialog"
        aria-modal="true"
        aria-label="Lead details"
      >
        <div className="drawer-top">
          <span className="eyebrow">
            INQUIRY DETAILS {lead.is_demo ? "· DEMO" : ""}
          </span>
          <button aria-label="Close details" onClick={close}>
            <X size={20} />
          </button>
        </div>
        <h2>{lead.name}</h2>
        <p>{lead.contact}</p>
        <div className="detail-grid">
          <div>
            <small>Source</small>
            <strong>{lead.source}</strong>
          </div>
          <div>
            <small>Campaign</small>
            <strong>{lead.campaign || "Not supplied"}</strong>
          </div>
          <div>
            <small>Channel</small>
            <strong>{label(lead.channel)}</strong>
          </div>
          <div>
            <small>Booked revenue</small>
            <strong>{money(lead.booked_minor)}</strong>
          </div>
        </div>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void run(async () => {
              await api(
                `/inquiries/${lead.id}`,
                { version: editVersion, qualification, notes },
                "PATCH",
              );

            });
          }}
        >
          <div className="form-field">
            <label htmlFor="lead-qualification">Qualification</label>
            <select
              id="lead-qualification"
              value={qualification}
              onChange={(e) => setQualification(e.target.value)}
            >
              {["unreviewed", "qualified", "unqualified", "spam"].map((v) => (
                <option key={v} value={v}>
                  {label(v)}
                </option>
              ))}
            </select>
          </div>
          <div className="form-field">
            <label htmlFor="lead-notes">Notes</label>
            <textarea
              id="lead-notes"
              rows={4}
              maxLength={4000}
              value={notes}
              onChange={(e) => setNotes(e.target.value)}
            />
          </div>
          <button className="primary" disabled={busy}>
            Save lead
          </button>
        </form>
        {dirty && <p className="notice">Save your review changes before grouping interactions or editing bookings.</p>}
        <InquiryGrouping inquiry={lead} api={api} disabled={busy || dirty} changed={changed} />
        <hr />
        <h3>Bookings</h3>
        <p className="small muted">
          Use the booking ID from your business system. Repeated IDs do not add
          revenue twice.
        </p>
        {outcomes.map((o) => (
          <div className="outcome" key={o.external_id}>
            <div>
              <strong>{o.external_id}</strong>
              <small>
                {money(o.amount_minor)} · {o.status} · revision {o.revision}
              </small>
            </div>
            {o.status === "booked" && (
              <button
                disabled={busy || dirty}
                className="text-button"
                onClick={() =>
                  void run(async () => {
                    await api(
                      `/inquiries/${lead.id}/outcomes`,
                      { ...o, status: "canceled", revision: o.revision + 1 },
                      "PUT",
                    );
                  })
                }
              >
                Cancel booking
              </button>
            )}
          </div>
        ))}
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void run(async () => {
              const parts = amount.split(".");
              const minor =
                Number(parts[0]) * 100 +
                Number((parts[1] || "").padEnd(2, "0"));
              await api(
                `/inquiries/${lead.id}/outcomes`,
                {
                  external_id: booking,
                  amount_minor: minor,
                  currency: "USD",
                  status: "booked",
                  revision: 1,
                },
                "PUT",
              );
              setBooking("");
              setAmount("");
            });
          }}
        >
          <div className="form-row">
            <label>
              Booking ID
              <input
                required
                maxLength={150}
                placeholder="booking-1042"
                value={booking}
                onChange={(e) => setBooking(e.target.value)}
              />
            </label>
            <label>
              Amount (USD)
              <input
                required
                inputMode="decimal"
                pattern="[0-9]+(\.[0-9]{1,2})?"
                placeholder="250.00"
                value={amount}
                onChange={(e) => setAmount(e.target.value)}
              />
            </label>
          </div>
          <button className="secondary" disabled={busy || dirty}>
            <Plus size={15} /> Record booking
          </button>
        </form>
        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}
        {notice && (
          <p className="success" role="status">
            {notice}
          </p>
        )}
      </section>
    </div>
  );
}
function CreateModal({
  kind,
  api,
  close,
  done,
  connected,
  signalwireConnected,
  businessProfile = false,
}: {
  kind: "number" | "lead";
  api: Client;
  close: () => void;
  done: () => Promise<void>;
  connected: boolean;
  signalwireConnected: boolean;
  businessProfile?: boolean;
}) {
  const [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  const [provider, setProvider] = useState<"twilio" | "signalwire">(!connected && signalwireConnected ? "signalwire" : "twilio");
  const [inventory, setInventory] = useState<TwilioInventory | null>(null);
  const [inventoryError, setInventoryError] = useState("");
  // Separate from the submit error: a renewed workspace token changes `api`, and reloading
  // the inventory must not wipe the message telling the user their last submit failed.
  useEffect(() => {
    let active = true;
    setInventory(null); setInventoryError("");
    if (kind === "number" && (provider === "twilio" ? connected : signalwireConnected)) {
      api<TwilioInventory>(`/connections/${provider}/numbers`).then(value => { if (active) setInventory(value); })
        .catch(error => { if (active) setInventoryError(error.message); });
    }
    return () => { active = false; };
  }, [kind, connected, signalwireConnected, provider, api]);
  const fields =
    kind === "number"
      ? [
          ["label", "Label", businessProfile ? "LA Google Business Profile" : "Google search line"],
          ["phone_number", provider === "twilio" ? "Twilio tracking number" : "SignalWire tracking number", "+12025550101"],
          ["destination", "Business destination", "+12025550102"],
          ["source", "Marketing source", "Google Ads"],
          ["campaign", businessProfile ? "Business Profile location" : "Campaign", businessProfile ? "Los Angeles" : "Spring campaign"],
        ]
      : [
          ["external_id", "Submission ID", "form-1042"],
          ["name", "Name", "Alex Morgan"],
          ["contact", "Email or phone", "alex@example.com"],
          ["source", "Marketing source", "Website"],
          ["campaign", "Campaign (optional)", "Contact form"],
        ];
  return (
    <div className="overlay modal-overlay">
      <section
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-label={kind === "number" ? businessProfile ? "Add Business Profile number" : "Add tracking number" : "Add form lead"}
      >
        <div className="drawer-top">
          <h2>{kind === "number" ? businessProfile ? "Add Business Profile number" : "Add tracking number" : "Add form lead"}</h2>
          <button aria-label="Close form" onClick={close}>
            <X size={20} />
          </button>
        </div>
        <p>
          {kind === "number"
            ? businessProfile ? "Register a dedicated static number for one Google listing. Keep it outside website pools. This does not change carrier routing or your Google listing." : "Register an existing provider number and where to forward calls."
            : "Record a submitted inquiry. The private API also accepts server-to-server form submissions."}
        </p>
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            const data = Object.fromEntries(new FormData(e.currentTarget));
            setBusy(true);
            setError("");
            try {
              await api(
                kind === "number" ? "/numbers" : "/leads",
                kind === "number" ? { ...data, provider } : { ...data, notes: "" },
              );
              await done();
            } catch (e) {
              setError((e as Error).message);
            } finally {
              setBusy(false);
            }
          }}
        >
          {kind === "number" && <label>Phone provider<select aria-label="Phone provider" value={provider} onChange={e => setProvider(e.target.value as "twilio" | "signalwire")} disabled={busy}><option value="twilio">Twilio</option><option value="signalwire" disabled={!signalwireConnected}>SignalWire{!signalwireConnected ? " — connect first" : ""}</option></select></label>}
          {fields.map(([name, title, placeholder], i) => (
            <label key={name}>
              {title}
              <input
                name={name}
                list={name === "phone_number" ? "twilio-owned-numbers" : undefined}
                placeholder={placeholder}
                defaultValue={businessProfile && name === "source" ? "Google Business Profile" : undefined}
                readOnly={businessProfile && name === "source"}
                required={!(kind === "lead" && name === "campaign")}
                autoFocus={i === 0}
                maxLength={name === "contact" ? 200 : 150}
              />
            </label>
          ))}
          {kind === "number" && <datalist id="twilio-owned-numbers">{inventory?.numbers.map(n => <option key={n.sid} value={n.phone_number}>{n.friendly_name}</option>)}</datalist>}
          {inventory?.has_more && <p className="small muted">Suggestions show the first page. Enter another owned number to verify it directly.</p>}
          {(error || inventoryError) && (
            <p className="error" role="alert">
              {error || inventoryError}
            </p>
          )}
          <button className="primary" disabled={busy}>
            {busy
              ? "Saving…"
              : kind === "number"
                ? "Register number"
                : "Add lead"}
          </button>
        </form>
      </section>
    </div>
  );
}
function Connections({ api, connection, signalwire, refresh }: { api: Client; connection: Connection; signalwire: SignalWireStatus | null; refresh: () => Promise<void> }) {
  const [url, setUrl] = useState(
      "https://example.com/?utm_source=google&utm_medium=cpc&utm_campaign=spring",
    ),
    [preview, setPreview] = useState(""),
    [ready, setReady] = useState(false);
  useEffect(() => {
    initWasm()
      .then(() => setReady(true))
      .catch(() => setPreview("Could not load the attribution preview."));
  }, []);
  return (
    <div className="connections">
      <TwilioConnection api={api} connection={connection} refresh={refresh} />
      {signalwire && <SignalWireConnection api={api} connection={signalwire} refresh={refresh} />}
      <section className="card connection">
        <div className="card-header">
          <div>
            <h2>Attribution preview</h2>
            <p>Shared Rust rules, running in your browser through WASM.</p>
          </div>
          <span className="chip">UTM evidence</span>
        </div>
        <form
          className="connection-body"
          onSubmit={(e) => {
            e.preventDefault();
            try {
              const p = JSON.parse(preview_attribution(url));
              setPreview(
                `Source: ${p.source || "unknown"} · Medium: ${p.medium || "unknown"} · Campaign: ${p.campaign || "unknown"}`,
              );
            } catch (e) {
              setPreview(String(e));
            }
          }}
        >
          <label>
            Landing page URL
            <input
              type="url"
              required
              value={url}
              onChange={(e) => setUrl(e.target.value)}
            />
          </label>
          <button className="secondary" disabled={!ready}>
            Preview attribution <ArrowRight size={15} />
          </button>
          {preview && (
            <p role="status" className="preview-result">
              {preview}
            </p>
          )}
          <p className="small muted">
            Missing UTM evidence stays unknown. This preview does not install a
            website tracker.
          </p>
        </form>
      </section>
      <section className="next-up">
        <span className="eyebrow">COMING AFTER THE PILOT</span>
        <h3>More context, fewer gaps.</h3>
        <p>
          Website capture is available in Sites. Session number pools, Google
          conversion delivery, and VisiComms connections are planned next.
        </p>
      </section>
    </div>
  );
}
