#!/usr/bin/env python3
"""Lists the third-party dependencies of Caelumora with their licenses.

    generate.py [--bundle [FILE]]

Without arguments, writes BACKEND.md (the Rust crates of the workspace, from Cargo.lock) and
FRONTEND.md (the npm packages of caelumora-web-ui, from package-lock.json) next to this script.
Both are generated and never edited by hand.

With --bundle, writes the license bundle instead: every dependency that goes into the program,
with the license files it ships, as one text file to be distributed with the program. It is not
checked in; by default it is written to target/THIRD-PARTY-LICENSES.txt. The bundle needs the
installed packages of the web UI (`npm ci` in caelumora-web-ui), and for a dependency that ships
no license file of its own it downloads the standard text of its license from the SPDX license
list.
"""
import argparse
import collections
import json
import os
import re
import subprocess
import sys
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
WEB_UI = os.path.join(ROOT, "caelumora-web-ui")
SPDX_TEXT = "https://raw.githubusercontent.com/spdx/license-list-data/main/text/{}.txt"

LICENSE_FILE = re.compile(r"^(licen[cs]e|copying|notice|unlicense|copyright|third.party)", re.I)
PERMISSIVE = {
    "MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "BSD-1-Clause", "ISC", "Unlicense",
    "BSL-1.0", "Unicode-3.0", "Zlib", "0BSD", "CC0-1.0", "BlueOak-1.0.0",
    "Apache-2.0 WITH LLVM-exception",
}


def normalize(expr):
    """Writes the old Cargo form `MIT/Apache-2.0` as an SPDX expression."""
    if expr and "/" in expr and " OR " not in expr:
        return " OR ".join(part.strip() for part in expr.split("/"))
    return expr or "(not specified)"


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


def cargo_dependencies():
    """The crates the workspace depends on, one entry per crate name."""
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked"],
        cwd=ROOT, check=True, capture_output=True, text=True).stdout)
    members = set(meta["workspace_members"])
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}

    runtime, dev, direct = set(), set(), set()
    queue = []
    for member in members:
        for dep in nodes[member]["deps"]:
            kinds = {k["kind"] for k in dep["dep_kinds"]}
            if dep["pkg"] not in members:
                direct.add(dep["pkg"])
            if kinds - {"dev"}:
                queue.append((dep["pkg"], runtime))
            if "dev" in kinds:
                queue.append((dep["pkg"], dev))
    while queue:
        pkg, target = queue.pop()
        if pkg in members or pkg in target:
            continue
        target.add(pkg)
        queue += [(d["pkg"], target) for d in nodes[pkg]["deps"]]

    by_name = collections.defaultdict(list)
    for pkg_id, pkg in packages.items():
        if pkg_id not in members:
            by_name[pkg["name"]].append(pkg)
    for name, versions in sorted(by_name.items()):
        versions.sort(key=lambda p: version_key(p["version"]))
        ids = {p["id"] for p in versions}
        latest = versions[-1]
        yield {
            "name": name,
            "versions": [p["version"] for p in versions],
            "license": "; ".join(sorted({normalize(p["license"]) for p in versions})),
            "source": repository_url(latest.get("repository")) or latest.get("homepage"),
            "usage": "runtime" if ids & runtime else "tests",
            "shipped": bool(ids & runtime),
            "direct": bool(ids & direct),
            "directories": [(p["version"], os.path.dirname(p["manifest_path"]))
                            for p in versions],
        }


def npm_dependencies():
    """The npm packages of the web UI, one entry per package name."""
    with open(os.path.join(WEB_UI, "package-lock.json"), encoding="utf-8") as f:
        lock = json.load(f)
    with open(os.path.join(WEB_UI, "package.json"), encoding="utf-8") as f:
        manifest = json.load(f)
    direct = set(manifest.get("dependencies", {})) | set(manifest.get("devDependencies", {}))

    by_name = collections.defaultdict(list)
    for path, pkg in lock["packages"].items():
        if path:
            by_name[pkg.get("name") or path.rsplit("node_modules/", 1)[1]].append((path, pkg))
    for name, versions in sorted(by_name.items()):
        shipped = not all(pkg.get("dev") for (_, pkg) in versions)
        usage = "runtime" if shipped else "build"
        if all(pkg.get("optional") or pkg.get("devOptional") for (_, pkg) in versions):
            usage += ", optional"
        yield {
            "name": name,
            "versions": sorted({pkg["version"] for (_, pkg) in versions}, key=version_key),
            "license": "; ".join(sorted({pkg.get("license") or "(not specified)"
                                         for (_, pkg) in versions})),
            # Not the repository: that is known only from the installed package, which differs
            # between platforms, and the list must not.
            "source": f"https://www.npmjs.com/package/{name}",
            "usage": usage,
            "shipped": shipped,
            "direct": name in direct,
            "directories": [(pkg["version"], os.path.join(WEB_UI, path))
                            for (path, pkg) in versions],
        }


def row(cells):
    return "| " + " | ".join(str(c).replace("|", "\\|") for c in cells) + " |"


def write_list(file, title, intro, usages, entries):
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
    lines += [row([f"`{e['name']}`", ", ".join(e["versions"]), f"`{e['license']}`", e["usage"],
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
        list(cargo_dependencies()))
    write_list(
        "FRONTEND.md", "Dependencies of the web UI",
        ["Every npm package of `caelumora-web-ui`, from `package-lock.json`. _Direct_ means",
         "`package.json` names the package. Generated by `generate.py`; do not edit by hand."],
        {"runtime": "goes into the web UI that is shipped",
         "build": "needed for building and development only",
         "optional": "depends on the platform, installed only there"},
        list(npm_dependencies()))


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
    rule = "=" * 80
    lines = [
        "THIRD-PARTY LICENSES",
        "",
        "Caelumora includes the following third-party software. Each package is listed with its",
        "license and the license files it ships; a package that ships none is followed by the",
        "standard text of the licenses it names.",
        "",
    ]
    missing, standard = [], {}
    for (title, entries) in [("Rust crates", cargo_dependencies()),
                             ("Web UI (npm packages)", npm_dependencies())]:
        lines += [rule, title.upper(), rule, ""]
        for entry in entries:
            if not entry["shipped"]:
                continue
            texts, seen, present = [], set(), False
            for (version, directory) in entry["directories"]:
                files = license_files(directory)
                if files is None:
                    continue
                present = True
                for (name, text) in files:
                    if text not in seen:
                        seen.add(text)
                        label = f"{name} ({version})" if len(entry["versions"]) > 1 else name
                        texts.append((label, text))
            if not present:
                missing.append(entry["name"])
                continue
            lines += ["-" * 80, f"{entry['name']} {', '.join(entry['versions'])}",
                      f"License: {entry['license']}"]
            if entry["source"]:
                lines.append(f"Source: {entry['source']}")
            lines.append("-" * 80)
            if not texts:
                for license_id in sorted(license_ids(entry["license"])):
                    texts.append((f"{license_id} (standard text)",
                                  standard_text(license_id, standard)))
            for (label, text) in texts:
                lines += ["", f"{label}:", "", text]
            lines.append("")
    if missing:
        sys.exit("packages are not installed (run `npm ci` in caelumora-web-ui): "
                 + ", ".join(missing))
    os.makedirs(os.path.dirname(file), exist_ok=True)
    with open(file, "w", encoding="utf-8") as out:
        out.write("\n".join(lines))
    print(f"wrote {file}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--bundle", nargs="?", metavar="FILE",
                        const=os.path.join(ROOT, "target", "THIRD-PARTY-LICENSES.txt"),
                        help="write the license bundle (default: %(const)s)")
    args = parser.parse_args()
    if args.bundle:
        write_bundle(os.path.abspath(args.bundle))
    else:
        write_lists()


if __name__ == "__main__":
    main()
