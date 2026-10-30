export type Account = { id: string; email: string; email_verified: boolean };
export type CloudSession = {
  csrf_token: string;
  user: Account | null;
  signups_enabled: boolean;
  billing: { enabled: boolean; mode: "test" | "live"; amount_minor: number; currency: string };
};
export type Workspace = {
  id: string; name: string; status: "pending" | "ready" | "failed";
  paid_access: boolean; can_manage_billing: boolean; billing_required: boolean;
};
export type Billing = {
  workspace: { id: string; name: string }; required: boolean; enabled: boolean;
  mode: "test" | "live"; amount_minor: number; currency: string; status: string;
  paid_access: boolean; cancellation_scheduled: boolean; access_until: string | null;
  can_checkout: boolean; can_manage: boolean;
  carrier_migration?: { state: "prepared" | "activating" | "active"; activated_at: string | null } | null;
};
export class CloudError extends Error {
  constructor(message: string, public status: number) { super(message); }
}
// Mail links carry their one-use credential in a fragment, never a server URL.
// Capture once before any fetch or React StrictMode mount, then remove it from history.
let accountLinkToken = "";
if (["/password_reset/edit", "/email_verification/edit"].includes(window.location.pathname)) {
  const url = new URL(window.location.href);
  accountLinkToken = new URLSearchParams(url.hash.slice(1)).get("token") || url.searchParams.get("token") || "";
  url.hash = ""; url.searchParams.delete("token");
  window.history.replaceState(null, "", url.pathname + url.search);
}
export const getAccountLinkToken = () => accountLinkToken;
export function clearAccountLinkToken() { accountLinkToken = ""; }
export async function cloudRequest<T>(path: string, csrf = "", body?: unknown, method = body ? "POST" : "GET"): Promise<T> {
  const response = await fetch(`/api/cloud${path}`, {
    method, credentials: "same-origin", redirect: "error",
    headers: { Accept: "application/json", ...(body ? { "Content-Type": "application/json" } : {}),
      ...(method !== "GET" ? { "X-CSRF-Token": csrf } : {}) },
    body: body ? JSON.stringify(body) : undefined,
  });
  const data = await response.json().catch(() => ({ error: "Account services are unavailable. Try again shortly." }));
  if (!response.ok) throw new CloudError(data.error || "The request could not be completed.", response.status);
  return data;
}
export async function engineAuth<T>(path: string, body: unknown): Promise<T> {
  const response = await fetch(`/auth/${path}`, {
    method: "POST", credentials: "same-origin", redirect: "error",
    headers: { "Content-Type": "application/json" }, body: JSON.stringify(body),
  });
  const data = await response.json();
  if (!response.ok) throw new Error(data.error || "Unable to open this workspace.");
  return data;
}
export function navigate(path: string, replace = false) {
  if (!path.startsWith("/") || path.startsWith("//")) throw new Error("Invalid app destination");
  if (replace) window.history.replaceState(null, "", path);
  else window.history.pushState(null, "", path);
  window.dispatchEvent(new PopStateEvent("popstate"));
}
