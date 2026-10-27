import { useEffect, useState } from "react";
import App from "./App";
import HostedApp from "./HostedApp";

export default function Root() {
  const [mode, setMode] = useState<"loading" | "hosted" | "standalone" | "error">("loading");
  useEffect(() => {
    let active = true;
    void fetch("/runtime.json", { credentials: "same-origin", cache: "no-store" })
      .then(async response => {
        if (!response.ok) throw new Error("Runtime configuration unavailable");
        const config = await response.json();
        // Legacy engine hostname remains usable during migration. Self-hosts need no Rails service.
        if (active) setMode(config.hosted_app_origin === window.location.origin ? "hosted" : "standalone");
      }).catch(() => { if (active) setMode("error"); });
    return () => { active = false; };
  }, []);
  if (mode === "hosted") return <HostedApp />;
  if (mode === "standalone") return <App />;
  return <main className="cloud-loading"><h1>VisiConvert</h1><p role="status">{mode === "error" ? "We couldn’t load the app. Please try again." : "Opening VisiConvert…"}</p>{mode === "error" && <button className="primary" onClick={() => window.location.reload()}>Try again</button>}</main>;
}
