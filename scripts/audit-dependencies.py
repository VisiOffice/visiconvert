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
    return finding == ("crates.io", "block-buffer", "0.10.4", "GHSA-qwgh-2vcv-g2f7")


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
