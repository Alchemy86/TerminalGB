#!/usr/bin/env python3
"""docs_index — every page in `docs/` must be reachable from the index.

The swarm view is why this exists. `docs/swarm-view.md` was a good page and an
*island*: nothing enumerated it, no other page mentioned it, and the only way to
find it was to already know it was there. It was not alone — when this tool was
written, **57 of 101 pages** were in no index at all.

The failure mode is quiet, which is what makes it worth gating. A page nobody
links to is not broken, does not fail a build, and reads perfectly well to
whoever wrote it. It simply never gets read again.

So: `docs/README.md` is the index, and this tool holds it to three rules.

1.  **Coverage** - every `docs/**/*.md` is linked from the index, or is on
    `EXEMPT` below with a reason. Adding a page and not indexing it is red.
2.  **Resolution** - every page the index links to exists. A renamed file that
    left a stale row behind is red. (`codemap_links.py --check` verifies links
    across the whole docs tree; this is the same check aimed at the one file
    where a broken link means a page has become unreachable.)
3.  **Description** - an index row must actually say what the page is for. A
    bare link added only to silence rule 1 is red.

4.  **No exempt page on the front page** - the top-level `README.md` may not
    link something `EXEMPT` claims is too marginal to index. That is the only
    way `EXEMPT` can be abused: silence rule 1 for a page, then promote it
    anyway. Rule 1 already covers everything else the README could link, so
    this is the one cross-file case worth its own check.

The top-level README's own "Every page, and what it is for" section stays a
*curated* selection with long descriptions - the shop window. It answers "why
would I read this?" for a visitor; this index answers "what is there?" for
someone already inside, so the two describe a page differently on purpose.
There is deliberately no generation step between them: only their *coverage* is
mechanical, and coverage is the half that broke.

Usage:
    docs_index.py            # report coverage
    docs_index.py --check    # same, and this is what `make docs-check` runs
"""
from __future__ import annotations

import argparse
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", ".."))  # docs/tools -> repo root
DOCS = os.path.join(ROOT, "docs")
INDEX = os.path.join(DOCS, "README.md")
README = os.path.join(ROOT, "README.md")

# Pages deliberately absent from the index. Each needs a reason, because the
# whole point of the tool is that "not indexed" should be a decision rather
# than an oversight.
EXEMPT = {
    # The index itself.
    "docs/README.md": "is the index",
    # Retired brand studies. Its own first line is "do not ship anything in
    # this directory" - indexing it would invite exactly that.
    "docs/brand/exploration/README.md": "retired design studies, explicitly not shipped",
}

# A markdown link whose target is a .md file, with the link text captured.
LINK_RE = re.compile(r"\[(?P<text>[^\]]+)\]\((?P<href>[^)\s]+\.md)(?:#[^)]*)?\)")

# How much a row has to say before it counts as describing the page. Short
# enough to allow "the four algorithms and their costs", long enough that a
# bare link cannot pass.
MIN_DESCRIPTION = 20


def docs_pages():
    """Every markdown page under docs/, repo-relative with forward slashes."""
    found = set()
    for dirpath, _dirnames, filenames in os.walk(DOCS):
        for name in filenames:
            if name.endswith(".md"):
                full = os.path.join(dirpath, name)
                found.add(os.path.relpath(full, ROOT).replace(os.sep, "/"))
    return found


def links_in(path, base):
    """(target, link text, whole line) for every .md link in one file.

    `base` is the directory link hrefs are relative to - `docs/` for the index,
    the repo root for the top-level README.
    """
    out = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            for m in LINK_RE.finditer(line):
                href = m.group("href")
                if "://" in href:
                    continue
                target = os.path.normpath(os.path.join(base, href))
                out.append((
                    os.path.relpath(target, ROOT).replace(os.sep, "/"),
                    m.group("text"),
                    line.rstrip("\n"),
                ))
    return out


def description_of(line):
    """The prose a table row offers about the page it links to.

    Rows are `| [title](path) | what it is |`, so the description is whatever
    survives stripping the cell that holds the link. A row that is not a table
    row (a bullet, a sentence) is measured on what is left after the link text.
    """
    if line.lstrip().startswith("|"):
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        rest = [c for c in cells if not LINK_RE.search(c)]
        return " ".join(rest).strip()
    return LINK_RE.sub("", line).strip(" |*-\t")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true",
                    help="verify only (the default behaviour; accepted for "
                         "symmetry with the other docs tools)")
    ap.parse_args()

    problems = []

    if not os.path.isfile(INDEX):
        print("FAIL - docs/README.md is missing; there is no documentation index")
        return 1

    pages = docs_pages()
    index_links = links_in(INDEX, DOCS)

    # Rule 2 - every row points at something.
    indexed = {}
    for target, _text, line in index_links:
        if target.startswith("docs/") and target not in pages:
            problems.append("docs/README.md links to %s, which does not exist" % target)
            continue
        indexed.setdefault(target, line)

    # Rule 1 - every page is on a row.
    for page in sorted(pages):
        if page in indexed or page in EXEMPT:
            continue
        problems.append(
            "%s is in no index - add a row to docs/README.md (or to EXEMPT in "
            "docs/tools/docs_index.py, with the reason)" % page
        )

    # An EXEMPT entry that no longer names a real page is stale.
    for page, reason in sorted(EXEMPT.items()):
        if page not in pages:
            problems.append(
                "EXEMPT lists %s (%s), which no longer exists" % (page, reason)
            )

    # Rule 3 - a row has to say something.
    for page, line in sorted(indexed.items()):
        if not page.startswith("docs/"):
            continue
        text = description_of(line)
        if len(text) < MIN_DESCRIPTION:
            problems.append(
                "docs/README.md: the row for %s says nothing useful (%d chars) - "
                "a link on its own is not an index entry" % (page, len(text))
            )

    # Rule 4 - nothing exempt may be on the front page.
    for target, text, _line in links_in(README, ROOT):
        if target in EXEMPT and target != "docs/README.md":
            problems.append(
                "README.md links to %s (\"%s\"), but it is EXEMPT from the index "
                "as \"%s\" - a page cannot be both too marginal to index and "
                "worth the front page. Index it, or stop linking it."
                % (target, text, EXEMPT[target])
            )

    if problems:
        print("FAIL:")
        for p in sorted(set(problems)):
            print("  -", p)
        return 1

    covered = sum(1 for p in indexed if p.startswith("docs/"))
    print("OK - %d page(s) under docs/: %d indexed, %d exempt. Every one is "
          "reachable from docs/README.md." % (len(pages), covered, len(EXEMPT)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
