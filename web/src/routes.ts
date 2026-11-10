// Page URLs inside a workspace. Hosted workspaces live under /app/<workspace id>;
// self-hosted installs use the same paths from the site root.
export type Page = "Setup" | "Overview" | "Lead inbox" | "Tracking numbers" | "Sites" | "Connections" | "Conversions" | "API keys" | "Webhooks" | "Logs";
export type Route = { page: Page; tab: string | null };

const slugs: Record<Page, string> = {
  Overview: "",
  Setup: "setup",
  "Lead inbox": "leads",
  "Tracking numbers": "numbers",
  Sites: "sites",
  Conversions: "conversions",
  Connections: "connections",
  "API keys": "api-keys",
  Webhooks: "webhooks",
  Logs: "logs",
};
const pages = Object.fromEntries(Object.entries(slugs).map(([page, slug]) => [slug, page])) as Record<string, Page>;
/// Tabs a page accepts in its URL; anything else opens the page's first tab.
const tabs: Partial<Record<Page, string[]>> = { Conversions: ["actions", "google", "microsoft", "meta", "events", "advanced"] };

const hostedBase = /^\/app\/[0-9a-fA-F-]{36}/;
export function base(pathname = window.location.pathname) { return pathname.match(hostedBase)?.[0] ?? ""; }

/// Unknown paths open the overview rather than an error page.
export function parse(pathname = window.location.pathname): Route {
  const [slug = "", tab = null] = pathname.slice(base(pathname).length).split("/").filter(Boolean);
  const page = pages[slug] ?? "Overview";
  return { page, tab: tab && tabs[page]?.includes(tab) ? tab : null };
}

export function href(route: Route, prefix = base()) {
  const parts = [slugs[route.page], route.tab].filter(Boolean);
  return parts.length ? `${prefix}/${parts.join("/")}` : prefix || "/";
}

/// Records a page change in browser history. Replace when the change is not a navigation the
/// user would expect Back to undo (for example correcting an unknown path).
export function go(route: Route, replace = false) {
  const next = href(route) + window.location.search;
  if (next === window.location.pathname + window.location.search) return;
  if (replace) window.history.replaceState(null, "", next);
  else window.history.pushState(null, "", next);
}
