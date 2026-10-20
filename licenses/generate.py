#!/usr/bin/env python3
"""Lists the third-party dependencies of Caelumora with their licenses.

    generate.py [--bundle [FILE]] [--sbom [FILE]]

Without arguments, writes BACKEND.md (the Rust crates of the workspace, from Cargo.lock) and
FRONTEND.md (the npm packages of caelumora-web-ui, from package-lock.json) next to this script.
Both are generated and never edited by hand.

With --bundle, writes the license bundle instead: every dependency that goes into the program,
with the license files it ships, as one text file to be distributed with the program. By default
it is written to target/THIRD-PARTY-LICENSES.txt. For a dependency that ships no license file of
its own it downloads the standard text of its license from the SPDX license list.

With --sbom, writes a software bill of materials in CycloneDX JSON: every dependency of the
backend and the web UI with its version, license, hashes and the dependencies between them;
those not going into the program have the scope `excluded`. By default it is written to
target/caelumora.cdx.json.

Neither the bundle nor the SBOM is checked in. Both need the installed packages of the web UI
(`npm ci` in caelumora-web-ui).
"""
import argparse
import base64
import collections
import datetime
import json
import os
import re
import subprocess
import sys
import tomllib
import urllib.request
import uuid

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
WEB_UI = os.path.join(ROOT, "caelumora-web-ui")
SPDX_TEXT = "https://raw.githubusercontent.com/spdx/license-list-data/main/text/{}.txt"
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"

LICENSE_FILE = re.compile(r"^(licen[cs]e|copying|notice|unlicense|copyright|third.party)", re.I)
PERMISSIVE = {
    "MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "BSD-1-Clause", "ISC", "Unlicense",
    "BSL-1.0", "Unicode-3.0", "Zlib", "0BSD", "CC0-1.0", "BlueOak-1.0.0",
    "Apache-2.0 WITH LLVM-exception",
}
UNSPECIFIED = "(not specified)"


def normalize(expr):
    """Writes the old Cargo form `MIT/Apache-2.0` as an SPDX expression."""
    if expr and "/" in expr and " OR " not in expr:
        return " OR ".join(part.strip() for part in expr.split("/"))
    return expr or UNSPECIFIED


def license_ids(expr):
    """The licenses an SPDX expression names, exceptions left out."""
    expr = re.sub(r"\s+WITH\s+\S+", "", expr)
    return {t for t in re.findall(r"[^\s()]+", expr) if t not in ("AND", "OR")}


def satisfiable_permissively(expr):
    """True if the expression can be met by choosing permissive licenses only."""
    tokens = re.findall(r"\(|\)|AND|OR|[^\s()]+(?:\s+WITH\s+[^\s()]+)?", expr)
    pos = 0

    def parse_or():
        nonlocal pos
        value = parse_and()
        while pos < len(tokens) and tokens[pos] == "OR":
            pos += 1
            value = parse_and() or value
        return value

    def parse_and():
        nonlocal pos
        value = parse_atom()
        while pos < len(tokens) and tokens[pos] == "AND":
            pos += 1
            value = parse_atom() and value
        return value

    def parse_atom():
        nonlocal pos
        token = tokens[pos]
        pos += 1
        if token == "(":
            value = parse_or()
            pos += 1
            return value
        return re.sub(r"\s+", " ", token) in PERMISSIVE

    return parse_or()


def version_key(version):
    return [(0, int(x), "") if x.isdigit() else (1, 0, x) for x in re.split(r"[.+-]", version)]


def repository_url(repo):
    """A browsable URL for the repository field of a manifest, or None."""
    if isinstance(repo, dict):
        repo = repo.get("url")
    if not repo:
        return None
    repo = re.sub(r"^git\+", "", repo)
    repo = re.sub(r"^git://", "https://", repo)
    repo = re.sub(r"^ssh://git@", "https://", repo)
    repo = re.sub(r"^git@([^:/]+):", r"https://\1/", repo)
    repo = re.sub(r"\.git$", "", repo)
    if re.match(r"^github:|^[\w.-]+/[\w.-]+$", repo):
        repo = "https://github.com/" + repo.removeprefix("github:")
    return repo


def license_files(directory):
    """The license files at the root of a package as (name, text), or None if it is not there."""
    if not os.path.isdir(directory):
        return None
    files = []
    for name in sorted(os.listdir(directory)):
        path = os.path.join(directory, name)
        if not LICENSE_FILE.match(name):
            continue
        paths = [(name, path)] if os.path.isfile(path) else [
            (f"{name}/{sub}", os.path.join(path, sub)) for sub in sorted(os.listdir(path))
        ] if os.path.isdir(path) else []
        for (label, file) in paths:
            if os.path.isfile(file):
                with open(file, encoding="utf-8", errors="replace") as f:
                    files.append((label, f.read().strip()))
    return files


def cargo_packages():
    """The workspace crates and the crates they depend on, one package per version.

    Answers (members, packages); a member has the fields of a part of Caelumora (`own_part`).
    """
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked"],
        cwd=ROOT, check=True, capture_output=True, text=True).stdout)
    with open(os.path.join(ROOT, "Cargo.lock"), "rb") as f:
        checksums = {(p["name"], p["version"], p.get("source")): p.get("checksum")
                     for p in tomllib.load(f)["package"]}
    member_ids = set(meta["workspace_members"])
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}

    runtime, direct = set(), set()
    queue = []
    for member in member_ids:
        for dep in nodes[member]["deps"]:
            if dep["pkg"] not in member_ids:
                direct.add(dep["pkg"])
            if {k["kind"] for k in dep["dep_kinds"]} - {"dev"}:
                queue.append(dep["pkg"])
    while queue:
        pkg = queue.pop()
        if pkg not in member_ids and pkg not in runtime:
            runtime.add(pkg)
            queue += [d["pkg"] for d in nodes[pkg]["deps"]]

    refs = {}
    for p in meta["packages"]:
        refs[p["id"]] = (f"{p['name']}@{p['version']}" if p["id"] in member_ids
                         else f"pkg:cargo/{p['name']}@{p['version']}")

    members, packages = [], []
    for p in meta["packages"]:
        depends = sorted({refs[d["pkg"]] for d in nodes[p["id"]]["deps"]})
        if p["id"] in member_ids:
            is_application = any("bin" in t["kind"] for t in p["targets"])
            members.append(own_part(p["name"], p["version"], refs[p["id"]], is_application,
                                    p["license"], depends))
            continue
        checksum = checksums.get((p["name"], p["version"], p["source"]))
        packages.append(dict(
            ecosystem="cargo", name=p["name"], version=p["version"],
            license=normalize(p["license"]),
            repository=repository_url(p.get("repository")), homepage=p.get("homepage"),
            download=(f"https://crates.io/api/v1/crates/{p['name']}/{p['version']}/download"
                      if p["source"] == CRATES_IO else None),
            purl=refs[p["id"]],
            hashes=[("SHA-256", checksum)] if checksum else [],
            usage="runtime" if p["id"] in runtime else "tests",
            shipped=p["id"] in runtime, direct=p["id"] in direct,
            directory=os.path.dirname(p["manifest_path"]), depends=depends))
    return sorted(members, key=lambda m: m["name"]), packages


def own_part(name, version, ref, is_application, license, depends):
    """A part of Caelumora itself: a crate of the workspace or the web UI."""
    return {"name": name, "version": version, "ref": ref,
            "type": "application" if is_application else "library",
            "license": license or UNSPECIFIED, "depends": depends}


def npm_purl(name, version):
    if name.startswith("@"):
        scope, short = name[1:].split("/", 1)
        return f"pkg:npm/%40{scope}/{short}@{version}"
    return f"pkg:npm/{name}@{version}"


def integrity_hashes(integrity):
    """The hashes of an npm `integrity` field, e.g. `sha512-<base64>`, as (algorithm, hex)."""
    algorithms = {"sha1": "SHA-1", "sha256": "SHA-256", "sha384": "SHA-384", "sha512": "SHA-512"}
    hashes = []
    for part in (integrity or "").split():
        algorithm, _, digest = part.partition("-")
        if algorithm in algorithms:
            hashes.append((algorithms[algorithm], base64.b64decode(digest).hex()))
    return hashes


def npm_packages():
    """The web UI and the npm packages it depends on, one package per version.

    Answers (web_ui, packages); the web UI has the fields of a part of Caelumora (`own_part`).
    """
    with open(os.path.join(WEB_UI, "package-lock.json"), encoding="utf-8") as f:
        lock = json.load(f)["packages"]

    def name_of(path):
        return lock[path].get("name") or path.rsplit("node_modules/", 1)[1]

    def resolve(path, dependency):
        """The path a dependency of the package at `path` resolves to, as Node finds it."""
        while True:
            candidate = f"{path}/node_modules/{dependency}" if path else \
                f"node_modules/{dependency}"
            if candidate in lock:
                return candidate
            if not path:
                return None
            path = path.rsplit("/node_modules/", 1)[0] if "/node_modules/" in path else ""

    def depends(path):
        entry = lock[path]
        names = set()
        for field in ("dependencies", "devDependencies", "optionalDependencies",
                      "peerDependencies"):
            names |= set(entry.get(field, {}))
        resolved = (resolve(path, n) for n in names)
        return {npm_purl(name_of(r), lock[r]["version"]) for r in resolved if r}

    root = lock[""]
    direct = set(root.get("dependencies", {})) | set(root.get("devDependencies", {}))
    web_ui = own_part(root["name"], root["version"], f"{root['name']}@{root['version']}", True,
                      root.get("license"), sorted(depends("")))

    paths_by_purl = collections.defaultdict(list)
    for path, entry in lock.items():
        if path:
            paths_by_purl[npm_purl(name_of(path), entry["version"])].append(path)
    packages = []
    for purl, paths in paths_by_purl.items():
        entries = [lock[path] for path in paths]
        name = name_of(paths[0])
        # A package found at several paths is installed at some of them only.
        path = next((p for p in paths if os.path.isdir(os.path.join(WEB_UI, p))), paths[0])
        installed = {}
        try:
            with open(os.path.join(WEB_UI, path, "package.json"), encoding="utf-8") as f:
                installed = json.load(f)
        except OSError:
            pass
        shipped = any(not e.get("dev") for e in entries)
        usage = "runtime" if shipped else "build"
        if all(e.get("optional") or e.get("devOptional") for e in entries):
            usage += ", optional"
        packages.append(dict(
            ecosystem="npm", name=name, version=lock[path]["version"],
            license=lock[path].get("license") or UNSPECIFIED,
            repository=repository_url(installed.get("repository")),
            homepage=installed.get("homepage"), download=lock[path].get("resolved"), purl=purl,
            hashes=integrity_hashes(lock[path].get("integrity")), usage=usage, shipped=shipped,
            direct=f"node_modules/{name}" in paths and name in direct,
            directory=os.path.join(WEB_UI, path),
            depends=sorted(set().union(*(depends(p) for p in paths)))))
    return web_ui, packages


def by_name(packages):
    """The packages grouped by name, as (name, versions) sorted by name and version."""
    groups = collections.defaultdict(list)
    for p in packages:
        groups[p["name"]].append(p)
    return [(name, sorted(versions, key=lambda p: version_key(p["version"])))
            for name, versions in sorted(groups.items())]


def row(cells):
    return "| " + " | ".join(str(c).replace("|", "\\|") for c in cells) + " |"


def write_list(file, title, intro, usages, packages, source):
    entries = []
    for name, versions in by_name(packages):
        usage = sorted({p["usage"] for p in versions}, key=lambda u: not u.startswith("runtime"))
        entries.append({
            "name": name,
            "versions": ", ".join(p["version"] for p in versions),
            "license": "; ".join(sorted({p["license"] for p in versions})),
            "usage": usage[0],
            "direct": any(p["direct"] for p in versions),
            "source": source(versions[-1]),
        })
    lines = [f"# {title}", "", *intro, "", "The column _Usage_:", ""]
    lines += [f"*   _{k}_: {v}" for k, v in usages.items()]
    lines += ["", "## Licenses at a glance", ""]
    counts = collections.Counter(e["license"] for e in entries)
    lines += [row(["License", "Packages"]), "|---|---|"]
    lines += [row([f"`{lic}`", n]) for lic, n in counts.most_common()]
    lines.append("")
    flagged = [e for e in entries
               if not all(satisfiable_permissively(l) for l in e["license"].split("; "))]
    if flagged:
        lines += ["Not purely permissive:", ""]
        lines += [f"*   `{e['name']}`: `{e['license']}`, {e['usage']}" for e in flagged]
        lines.append("")
    lines += ["## Dependencies", ""]
    lines += [row(["Package", "Version", "License", "Usage", "Direct", "Source"]),
              "|---|---|---|---|:-:|---|"]
    lines += [row([f"`{e['name']}`", e["versions"], f"`{e['license']}`", e["usage"],
                   "✓" if e["direct"] else "", e["source"] or ""]) for e in entries]
    lines.append("")
    with open(os.path.join(HERE, file), "w", encoding="utf-8") as out:
        out.write("\n".join(lines))


def write_lists():
    write_list(
        "BACKEND.md", "Dependencies of the Rust backend",
        ["Every crate the crates of the workspace depend on, from `Cargo.lock`, for all target",
         "platforms. _Direct_ means a crate of the workspace names the package in its",
         "`Cargo.toml`. Generated by `generate.py`; do not edit by hand."],
        {"runtime": "goes into the programs (build scripts and macros included)",
         "tests": "needed for tests only"},
        cargo_packages()[1], lambda p: p["repository"] or p["homepage"])
    write_list(
        "FRONTEND.md", "Dependencies of the web UI",
        ["Every npm package of `caelumora-web-ui`, from `package-lock.json`. _Direct_ means",
         "`package.json` names the package. Generated by `generate.py`; do not edit by hand."],
        {"runtime": "goes into the web UI that is shipped",
         "build": "needed for building and development only",
         "optional": "depends on the platform, installed only there"},
        # Not the repository: that is known only from the installed package, which differs
        # between platforms, and the list must not.
        npm_packages()[1], lambda p: f"https://www.npmjs.com/package/{p['name']}")


def require_installed(packages):
    missing = sorted({p["name"] for p in packages
                      if p["shipped"] and not os.path.isdir(p["directory"])})
    if missing:
        sys.exit("packages are not installed (run `npm ci` in caelumora-web-ui): "
                 + ", ".join(missing))


def standard_text(license_id, cache):
    if license_id not in cache:
        url = SPDX_TEXT.format(license_id)
        try:
            with urllib.request.urlopen(url, timeout=30) as response:
                cache[license_id] = response.read().decode("utf-8").strip()
        except OSError as error:
            sys.exit(f"cannot download the standard text of {license_id} from {url}: {error}")
    return cache[license_id]


def write_bundle(file):
    cargo, npm = cargo_packages()[1], npm_packages()[1]
    require_installed(npm)
    rule = "=" * 80
    lines = [
        "THIRD-PARTY LICENSES",
        "",
        "Caelumora includes the following third-party software. Each package is listed with its",
        "license and the license files it ships; a package that ships none is followed by the",
        "standard text of the licenses it names.",
        "",
    ]
    standard = {}
    for (title, packages) in [("Rust crates", cargo), ("Web UI (npm packages)", npm)]:
        lines += [rule, title.upper(), rule, ""]
        for name, versions in by_name(p for p in packages if p["shipped"]):
            texts, seen = [], set()
            for p in versions:
                for (file_name, text) in license_files(p["directory"]) or []:
                    if text not in seen:
                        seen.add(text)
                        label = f"{file_name} ({p['version']})" if len(versions) > 1 \
                            else file_name
                        texts.append((label, text))
            license = "; ".join(sorted({p["license"] for p in versions}))
            source = versions[-1]["repository"] or versions[-1]["homepage"]
            lines += ["-" * 80, f"{name} {', '.join(p['version'] for p in versions)}",
                      f"License: {license}"]
            if source:
                lines.append(f"Source: {source}")
            lines.append("-" * 80)
            if not texts:
                for license_id in sorted(license_ids(license) - {UNSPECIFIED}):
                    texts.append((f"{license_id} (standard text)",
                                  standard_text(license_id, standard)))
            for (label, text) in texts:
                lines += ["", f"{label}:", "", text]
            lines.append("")
    os.makedirs(os.path.dirname(file), exist_ok=True)
    with open(file, "w", encoding="utf-8") as out:
        out.write("\n".join(lines))
    print(f"wrote {file}")


def cyclonedx_licenses(expr):
    if expr == UNSPECIFIED:
        return []
    if re.fullmatch(r"[A-Za-z0-9.+-]+", expr):
        return [{"license": {"id": expr}}]
    if re.search(r"\b(AND|OR|WITH)\b", expr):
        return [{"expression": expr}]
    return [{"license": {"name": expr}}]


def cyclonedx_component(p):
    component = {
        "type": "library",
        "bom-ref": p["purl"],
        "name": p["name"],
        "version": p["version"],
        "scope": "required" if p["shipped"] else "excluded",
        "purl": p["purl"],
    }
    if p["ecosystem"] == "npm" and p["name"].startswith("@"):
        component["group"], component["name"] = p["name"].split("/", 1)
    if p["hashes"]:
        component["hashes"] = [{"alg": alg, "content": hex} for (alg, hex) in p["hashes"]]
    if cyclonedx_licenses(p["license"]):
        component["licenses"] = cyclonedx_licenses(p["license"])
    references = [("vcs", p["repository"]), ("website", p["homepage"]),
                  ("distribution", p["download"])]
    references = [{"type": kind, "url": url} for (kind, url) in references if url]
    if references:
        component["externalReferences"] = references
    return component


def write_sbom(file):
    members, cargo = cargo_packages()
    web_ui, npm = npm_packages()
    require_installed(npm)
    parts = members + [web_ui]
    app = next(m for m in members if m["name"] == "caelumora-app")
    root = own_part("caelumora", app["version"], "caelumora", True, app["license"],
                    sorted(part["ref"] for part in parts))

    def own_component(part):
        component = {"type": part["type"], "bom-ref": part["ref"], "name": part["name"],
                     "version": part["version"]}
        if cyclonedx_licenses(part["license"]):
            component["licenses"] = cyclonedx_licenses(part["license"])
        return component

    components = [own_component(part) | {"scope": "required"} for part in parts]
    components += [cyclonedx_component(p) for p in sorted(cargo + npm, key=lambda p: p["purl"])]

    dependencies = [{"ref": part["ref"], "dependsOn": part["depends"]}
                    for part in [root] + parts]
    dependencies += [{"ref": p["purl"], "dependsOn": p["depends"]}
                     for p in sorted(cargo + npm, key=lambda p: p["purl"])]

    bom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.6",
        "serialNumber": f"urn:uuid:{uuid.uuid4()}",
        "version": 1,
        "metadata": {
            "timestamp": datetime.datetime.now(datetime.timezone.utc)
            .replace(microsecond=0).isoformat().replace("+00:00", "Z"),
            "tools": {"components": [{"type": "application", "name": "licenses/generate.py"}]},
            "component": own_component(root),
        },
        "components": components,
        "dependencies": dependencies,
    }
    os.makedirs(os.path.dirname(file), exist_ok=True)
    with open(file, "w", encoding="utf-8") as out:
        json.dump(bom, out, indent=2)
        out.write("\n")
    print(f"wrote {file}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--bundle", nargs="?", metavar="FILE",
                        const=os.path.join(ROOT, "target", "THIRD-PARTY-LICENSES.txt"),
                        help="write the license bundle (default: %(const)s)")
    parser.add_argument("--sbom", nargs="?", metavar="FILE",
                        const=os.path.join(ROOT, "target", "caelumora.cdx.json"),
                        help="write the SBOM in CycloneDX JSON (default: %(const)s)")
    args = parser.parse_args()
    if args.bundle:
        write_bundle(os.path.abspath(args.bundle))
    if args.sbom:
        write_sbom(os.path.abspath(args.sbom))
    if not args.bundle and not args.sbom:
        write_lists()


if __name__ == "__main__":
    main()
