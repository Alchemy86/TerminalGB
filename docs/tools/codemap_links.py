#!/usr/bin/env python3
"""codemap_links — keep the docs' code references clickable and honest.

The docs cite the source as ``path:line`` code spans inside Markdown links, e.g.

    [`src/cpu/core.rs:128`](../../src/cpu/core.rs)

A relative link like that opens the *file* in an editor or on GitHub, but it
cannot scroll to a line. This tool rewrites the *target* (never the visible
text) of every reference that carries a ``:line`` into a GitHub blob link with a
line anchor, so the reference actually navigates:

    [`src/cpu/core.rs:128`](https://github.com/Alchemy86/TerminalGB/blob/main/src/cpu/core.rs#L128)

Design decisions (see docs/codemap.md §"How the links stay honest"):

* **Branch links, not permalinks.** Targets point at ``blob/main`` so they track
  the code as it moves. The trade-off — a line number can drift silently — is
  covered by (a) a per-page "documented as of <sha>" marker this tool stamps,
  and (b) ``--check`` below, which fails when a referenced line no longer exists.
* **Only line references become absolute.** A link whose text has *no* ``:line``
  (a whole-file or whole-directory link, or a doc-to-doc link) is left relative,
  so it keeps working in a local editor as well as on GitHub.
* **Text is the source of truth for the line; the target for the path.** That
  makes the rewrite idempotent: run it on already-generated links and nothing
  changes. A maintainer edits the human-readable ``path:line`` and re-runs
  ``make docs-links``; URLs are never hand-edited.

Usage:
    codemap_links.py            # rewrite links + refresh markers (in place)
    codemap_links.py --check    # verify only; exit 1 on any broken reference
    codemap_links.py --sha SHA  # stamp markers with an explicit commit sha
"""
from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys

REPO = os.environ.get("CODEMAP_REPO", "Alchemy86/TerminalGB")
REF = os.environ.get("CODEMAP_REF", "main")

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", ".."))  # repo root (docs/tools -> ..)
DOCS = os.path.join(ROOT, "docs")

# Extensions we treat as "source we can line-anchor".
SRC_EXT = (".rs", ".go", ".h", ".toml", ".js", ".c", ".cpp", ".py", ".sh")

# Whole docs skipped by both --generate and --check: their references track a file
# another crew is actively rewriting, so precise line links would be wrong more often
# than right. They keep file-level relative links until the doc is refreshed.
# (docs/display.md's terminal-scaling refs follow src/screen/tui.rs.)
EXCLUDE = {"docs/display.md"}

# Source files whose refs are kept file-level (relative, no #Lnn) in EVERY doc,
# because the file is being rewritten right now and a line anchor would just churn.
# A file-level link still resolves; the `:line` in the visible text is a stale hint.
# Drop a file here and re-run `make docs-links` once it settles.
SRC_EXCLUDE = {"src/sound.rs", "src/screen/tui.rs"}

LINK = re.compile(r"\[(?P<text>[^\]]+)\]\((?P<target>[^)\s]+)\)")
CODESPAN = re.compile(r"`([^`]+)`")
BLOB = re.compile(r"^https?://github\.com/[^/]+/[^/]+/blob/[^/]+/(?P<path>[^#?]+)")
ASOF_SENTINEL = "<!-- codemap:asof -->"


def sh(*args: str) -> str:
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def first_line_in_text(text: str) -> int | None:
    """Return the first ``:line`` number found in the first code span of link text."""
    m = CODESPAN.search(text)
    if not m:
        return None
    lm = re.search(r":(\d+)", m.group(1))
    return int(lm.group(1)) if lm else None


def repo_path_for(target: str, md_dir: str) -> str | None:
    """Resolve a link target to a repo-relative path, or None if not a repo file."""
    b = BLOB.match(target)
    if b:
        return b.group("path")
    if target.startswith(("http://", "https://", "#", "mailto:")):
        return None
    # relative path: strip any #anchor, resolve against the markdown file's dir
    rel = target.split("#", 1)[0]
    abs_path = os.path.normpath(os.path.join(md_dir, rel))
    if not abs_path.startswith(ROOT):
        return None
    return os.path.relpath(abs_path, ROOT).replace(os.sep, "/")


def blob_url(path: str, line: int) -> str:
    return f"https://github.com/{REPO}/blob/{REF}/{path}#L{line}"


def process_links(text: str, md_dir: str, problems: list[str], md_rel: str):
    """Rewrite line-anchored source links; return (new_text, rewrite_count)."""
    count = 0

    def sub(m: re.Match) -> str:
        nonlocal count
        link_text, target = m.group("text"), m.group("target")
        line = first_line_in_text(link_text)
        if line is None:
            return m.group(0)  # no :line -> leave relative (editor-friendly)
        path = repo_path_for(target, md_dir)
        if path is None or not path.endswith(SRC_EXT):
            return m.group(0)
        if not os.path.isfile(os.path.join(ROOT, path)):
            problems.append(f"{md_rel}: target file missing: {path}")
            return m.group(0)
        if path in SRC_EXCLUDE:
            # keep it a file-level relative link (no line anchor) while the file churns
            rel = os.path.relpath(os.path.join(ROOT, path), md_dir).replace(os.sep, "/")
            new = f"[{link_text}]({rel})"
        else:
            new = f"[{link_text}]({blob_url(path, line)})"
        if new != m.group(0):
            count += 1
        return new

    return LINK.sub(sub, text), count


def marker_line(sha: str) -> str:
    short = sha[:7]
    tree = f"https://github.com/{REPO}/tree/{REF}"
    pin = f"https://github.com/{REPO}/tree/{sha}"
    return (
        f"> 🗺️ **Code map** · source links target [`{REF}`]({tree}); line references "
        f"documented as of commit [`{short}`]({pin}) · regenerate with "
        f"`make docs-links`, verify with `make docs-check`."
    )


def refresh_marker(text: str, sha: str) -> str:
    """Insert or update the per-page 'documented as of' marker after the first ---."""
    lines = text.split("\n")
    new = marker_line(sha)
    for i, ln in enumerate(lines):
        if ln.strip() == ASOF_SENTINEL:
            # replace the following blockquote line (or insert one)
            if i + 1 < len(lines) and lines[i + 1].startswith(">"):
                lines[i + 1] = new
            else:
                lines.insert(i + 1, new)
            return "\n".join(lines)
    # not present: insert a fresh block after the first horizontal rule. The rule
    # is already followed by a blank line, so don't add a trailing one.
    for i, ln in enumerate(lines):
        if ln.strip() == "---":
            lines[i + 1 : i + 1] = ["", ASOF_SENTINEL, new]
            return "\n".join(lines)
    return text  # no rule found; leave unmarked


def check_file(path: str, md_rel: str, problems: list[str]) -> int:
    """Verify every link in a file; return number of links checked."""
    text = open(path, encoding="utf-8").read()
    md_dir = os.path.dirname(path)
    checked = 0
    for m in LINK.finditer(text):
        target = m.group("target")
        rel_path = repo_path_for(target, md_dir)
        # GitHub blob links must match our repo and resolve to an existing line.
        b = BLOB.match(target)
        if b:
            checked += 1
            if f"github.com/{REPO}/blob/" not in target:
                problems.append(f"{md_rel}: blob link to a different repo: {target}")
                continue
            p = b.group("path")
            fp = os.path.join(ROOT, p)
            if not os.path.isfile(fp):
                problems.append(f"{md_rel}: blob path missing: {p}")
                continue
            lm = re.search(r"#L(\d+)", target)
            if lm:
                n = int(lm.group(1))
                total = sum(1 for _ in open(fp, encoding="utf-8", errors="replace"))
                if n < 1 or n > total:
                    problems.append(
                        f"{md_rel}: {p}#L{n} out of range (file has {total} lines)"
                    )
            continue
        # Relative links: the referenced file/dir must exist locally.
        if rel_path is not None:
            checked += 1
            fp = os.path.join(ROOT, rel_path)
            if not (os.path.isfile(fp) or os.path.isdir(fp)):
                problems.append(f"{md_rel}: relative link target missing: {rel_path}")
    return checked


def iter_md():
    for dirpath, _dirs, files in os.walk(DOCS):
        for f in sorted(files):
            if not f.endswith(".md"):
                continue
            path = os.path.join(dirpath, f)
            if os.path.relpath(path, ROOT).replace(os.sep, "/") in EXCLUDE:
                continue
            yield path


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true", help="verify only; do not write")
    ap.add_argument("--sha", help="commit sha for the 'documented as of' marker")
    args = ap.parse_args()

    problems: list[str] = []

    if args.check:
        total = 0
        for md in iter_md():
            total += check_file(md, os.path.relpath(md, ROOT), problems)
        if problems:
            print("FAIL — %d problem(s):" % len(problems))
            for p in problems:
                print("  -", p)
            return 1
        print(f"OK — verified {total} link(s) across the docs.")
        return 0

    sha = args.sha or sh("git", "rev-parse", "HEAD")
    changed_files = 0
    rewrites = 0
    for md in iter_md():
        md_rel = os.path.relpath(md, ROOT)
        md_dir = os.path.dirname(md)
        text = open(md, encoding="utf-8").read()
        new_text, n = process_links(text, md_dir, problems, md_rel)
        # Pages that carry a code line-link get the 'documented as of' marker.
        if n > 0 or ASOF_SENTINEL in new_text:
            new_text = refresh_marker(new_text, sha)
        if new_text != text:
            open(md, "w", encoding="utf-8").write(new_text)
            changed_files += 1
        rewrites += n
    print(
        f"Rewrote {rewrites} line reference(s); updated {changed_files} file(s); "
        f"stamped as of {sha[:7]}."
    )
    if problems:
        print("Warnings:")
        for p in problems:
            print("  -", p)
    return 0


if __name__ == "__main__":
    sys.exit(main())
