#!/usr/bin/env python3
"""Check locked public package versions against OSV; fail closed on lookup errors."""
import json
import re
import sys
import tomllib
import urllib.request
from pathlib import Path


def reviewed_nonapplicable(finding):
    # OSV's imported range omits the lower bound. The maintainer advisory says
    # >=0.11.0,<0.12.1; our 0.10.4 has a separate cursor, not inline storage.
    # https://github.com/RustCrypto/utils/security/advisories/GHSA-qwgh-2vcv-g2f7
    # Reviewed 2026-10-27. Scope this correction to the exact locked version;
    # other versions/advisories must still fail the audit.
    if finding == ("crates.io", "block-buffer", "0.10.4", "GHSA-qwgh-2vcv-g2f7"):
        return True
    # SQLite bug in the bundled C library (fixed in libsqlite3-sys 0.39.0). It is in
    # Cargo.lock only because sqlx 0.9.0 lists its SQLite driver as an optional
    # dependency and pins libsqlite3-sys below 0.39; with sqlx's `sqlite` feature off
    # it is never compiled (`cargo tree -i libsqlite3-sys` prints nothing). Reviewed
    # 2026-11-12. Holds only while no manifest enables SQLite; drop it when sqlx
    # allows libsqlite3-sys >= 0.39.
    if finding == ("crates.io", "libsqlite3-sys", "0.37.0", "RUSTSEC-2026-0459"):
        return not sqlite_enabled()
    return False


def sqlite_enabled():
    root = Path(__file__).resolve().parent.parent
    manifests = [root / "Cargo.toml", *root.glob("crates/*/Cargo.toml")]
    return any("sqlite" in manifest.read_text().lower() for manifest in manifests)


def packages(path):
    source = path.read_text()
    if path.name == "Cargo.lock":
        return [("crates.io", p["name"], p["version"])
                for p in tomllib.loads(source)["package"]
                if p.get("source", "").startswith("registry+")]
    if path.name == "Gemfile.lock":
        return [("RubyGems", name, version.split("-")[0])
                for name, version in re.findall(r"^    ([\w-]+) \(([^)]+)\)$", source, re.M)]
    if path.name == "pnpm-lock.yaml":
        return [("npm", name, version)
                for name, version in re.findall(r"^  '?(.+)@([0-9][^:\n']*)'?:$", source, re.M)
                if "(" not in version]
    raise ValueError(f"Unsupported lockfile: {path}")


def main():
    inventory = set()
    if len(sys.argv) < 2:
        raise ValueError("Pass Cargo.lock, Gemfile.lock, or pnpm-lock.yaml paths")
    for filename in sys.argv[1:]:
        found = packages(Path(filename))
        if not found:
            raise ValueError(f"No packages parsed from {filename}")
        inventory.update(found)
    queries = [{"package": {"ecosystem": ecosystem, "name": name}, "version": version}
               for ecosystem, name, version in sorted(inventory)]
    pending = queries
    findings = set()
    while pending:
        request = urllib.request.Request("https://api.osv.dev/v1/querybatch",
                                         data=json.dumps({"queries": pending}).encode(),
                                         headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(request, timeout=60) as response:
            results = json.load(response)["results"]
        if len(results) != len(pending):
            raise ValueError("Incomplete advisory response")
        more = []
        for query, result in zip(pending, results):
            for advisory in result.get("vulns", []):
                findings.add((query["package"]["ecosystem"], query["package"]["name"], query["version"], advisory["id"]))
            if result.get("next_page_token"):
                more.append({**query, "page_token": result["next_page_token"]})
        pending = more
    nonapplicable = {f for f in findings if reviewed_nonapplicable(f)}
    actionable = findings - nonapplicable
    print(json.dumps({"packages_checked": len(queries), "advisory_matches": sorted(actionable),
                      "reviewed_nonapplicable": sorted(nonapplicable)}, indent=2))
    return 1 if actionable else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        print(f"Dependency audit could not complete: {error}", file=sys.stderr)
        sys.exit(2)
