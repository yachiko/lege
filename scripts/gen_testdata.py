#!/usr/bin/env python3
"""Generate Markdown test data for Lege.

    python3 scripts/gen_testdata.py [OUT_DIR] [--huge-mb N]

Creates, under OUT_DIR (default: testdata/):

  baseline/   a realistic project's docs: a handful of normal-sized files,
              nested folders, an image, cross-links
  many/       ~2,000 small files in ~60 folders, including awkward names,
              a hidden folder and a .gitignored folder that Lege should skip
  huge/       one very large file (default ~10 MB) mixing every element type

Output is deterministic (fixed random seed) and OUT_DIR is wiped first.
Standard library only.
"""

import argparse
import random
import shutil
import struct
import zlib
from pathlib import Path

WORDS = """
the reader opens a document and follows each link through the folder until
every page has been read viewer render layout margin paper canvas sidebar
index section chapter note draft idea plan review release build config
option value default path file directory tree branch commit change update
fast slow small large simple clear quiet careful useful minimal precise
because while after before when where which that this these those some
many few most every often rarely always never only also still even just
""".split()

LANGS = {
    "rust": 'fn {name}(input: &str) -> usize {{\n    let words = input.split_whitespace();\n    // Count the words longer than {n} characters.\n    words.filter(|w| w.len() > {n}).count()\n}}',
    "python": 'def {name}(items: list[str], limit: int = {n}) -> dict[str, int]:\n    """Tally the first `limit` items."""\n    counts = {{}}\n    for item in items[:limit]:\n        counts[item] = counts.get(item, 0) + 1\n    return counts',
    "javascript": 'export async function {name}(url) {{\n  const response = await fetch(url, {{ cache: "no-store" }});\n  if (!response.ok) throw new Error(`HTTP ${{response.status}}`);\n  return (await response.json()).slice(0, {n});\n}}',
    "bash": '#!/usr/bin/env bash\nset -euo pipefail\nfor f in docs/*.md; do\n  wc -l "$f"  # {name}\ndone | sort -n | tail -n {n}',
    "json": '{{\n  "name": "{name}",\n  "version": "1.{n}.0",\n  "private": true,\n  "scripts": {{ "build": "make release" }}\n}}',
}

ALERTS = ["NOTE", "TIP", "IMPORTANT", "WARNING", "CAUTION"]


class Writer:
    def __init__(self, seed):
        self.rng = random.Random(seed)

    def words(self, lo, hi):
        return " ".join(self.rng.choice(WORDS) for _ in range(self.rng.randint(lo, hi)))

    def sentence(self):
        text = self.words(6, 18)
        return text[0].upper() + text[1:] + self.rng.choice(".....?!")

    def paragraph(self, lo=2, hi=6):
        sentences = [self.sentence() for _ in range(self.rng.randint(lo, hi))]
        # Sprinkle inline formatting.
        i = self.rng.randrange(len(sentences))
        style = self.rng.choice(["**{}**", "*{}*", "`{}`", "~~{}~~"])
        sentences[i] = sentences[i].replace(
            sentences[i].split()[1], style.format(sentences[i].split()[1]), 1
        )
        return " ".join(sentences)

    def title(self):
        return self.words(2, 5).title()

    def bullets(self, ordered=False, tasks=False):
        lines = []
        for i in range(self.rng.randint(3, 6)):
            marker = f"{i + 1}." if ordered else "-"
            if tasks:
                marker += " [x]" if self.rng.random() < 0.5 else " [ ]"
            lines.append(f"{marker} {self.words(3, 10)}")
            if self.rng.random() < 0.2:
                lines.append(f"   - {self.words(2, 6)}")
        return "\n".join(lines)

    def table(self):
        cols = self.rng.randint(2, 5)
        header = [self.words(1, 2).title() for _ in range(cols)]
        rows = [
            [self.rng.choice([self.words(1, 3), str(self.rng.randint(0, 9999)), "✓", "—"]) for _ in range(cols)]
            for _ in range(self.rng.randint(3, 8))
        ]
        lines = ["| " + " | ".join(header) + " |", "|" + "|".join([":---", ":---:", "---:"][i % 3] for i in range(cols)) + "|"]
        lines += ["| " + " | ".join(row) + " |" for row in rows]
        return "\n".join(lines)

    def code(self):
        lang = self.rng.choice(list(LANGS))
        body = LANGS[lang].format(name=self.rng.choice(["render", "scan", "tally", "fetch_page", "build"]), n=self.rng.randint(2, 20))
        return f"```{lang}\n{body}\n```"

    def alert(self):
        return f"> [!{self.rng.choice(ALERTS)}]\n> {self.sentence()}"

    def quote(self):
        return "> " + self.paragraph(1, 3)

    def block(self):
        kind = self.rng.choices(
            ["p", "ul", "ol", "tasks", "table", "code", "alert", "quote", "hr"],
            weights=[40, 10, 6, 3, 6, 12, 4, 4, 1],
        )[0]
        return {
            "p": self.paragraph,
            "ul": self.bullets,
            "ol": lambda: self.bullets(ordered=True),
            "tasks": lambda: self.bullets(tasks=True),
            "table": self.table,
            "code": self.code,
            "alert": self.alert,
            "quote": self.quote,
            "hr": lambda: "---",
        }[kind]()


def png(path, width, height, top=(106, 140, 175), bottom=(201, 214, 227)):
    """Write a vertical-gradient PNG using only zlib."""
    rows = bytearray()
    for y in range(height):
        t = y / max(height - 1, 1)
        color = bytes(round(a + (b - a) * t) for a, b in zip(top, bottom))
        rows += b"\x00" + color * width

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(bytes(rows), 9)) + chunk(b"IEND", b"")
    )


def write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text.rstrip() + "\n", encoding="utf-8")


def document(w, title, sections, links=(), level=1):
    h = "#" * level
    parts = [f"{h} {title}", w.paragraph()]
    for _ in range(sections):
        parts.append(f"{h}# {w.title()}")
        for _ in range(w.rng.randint(2, 5)):
            parts.append(w.block())
        if w.rng.random() < 0.4:
            parts.append(f"{h}## {w.title()}")
            parts.append(w.paragraph())
    if links:
        parts.append(f"{h}# See also")
        parts.append("\n".join(f"- [{label}]({target})" for label, target in links))
    return "\n\n".join(parts)


def baseline(out, w):
    """A small, realistic documentation set."""
    png(out / "images" / "banner.png", 960, 240)
    png(out / "images" / "diagram.png", 640, 360, top=(230, 236, 242), bottom=(170, 190, 212))

    write(
        out / "README.md",
        "\n\n".join(
            [
                "# Baseline Project",
                "![banner](images/banner.png)",
                w.paragraph(3, 5),
                "> [!NOTE]\n> This folder is generated by `make testdata`; edits are overwritten.",
                "## Contents",
                "- [Getting started](docs/getting-started.md)\n- [Configuration](docs/configuration.md)\n"
                "- [API reference](docs/api.md)\n- [Advanced tuning](docs/advanced/tuning.md)\n"
                "- [Changelog](CHANGELOG.md)\n- [Contributing](CONTRIBUTING.md)",
                "## Quick look",
                w.code(),
                w.table(),
                w.paragraph(),
                "Footnotes work too.[^1]",
                "[^1]: Rendered at the end of the page.",
            ]
        ),
    )
    docs = {
        "docs/getting-started.md": ("Getting Started", 5, [("Configuration", "configuration.md"), ("README", "../README.md")]),
        "docs/configuration.md": ("Configuration", 7, [("API reference", "api.md"), ("Tuning", "advanced/tuning.md")]),
        "docs/api.md": ("API Reference", 10, [("Getting started", "getting-started.md")]),
        "docs/advanced/tuning.md": ("Advanced Tuning", 6, [("Configuration", "../configuration.md")]),
        "CONTRIBUTING.md": ("Contributing", 4, [("README", "README.md")]),
    }
    for path, (title, sections, links) in docs.items():
        text = document(w, title, sections, links)
        if path.endswith("tuning.md"):
            text += "\n\n![diagram](../../images/diagram.png)"
        write(out / path, text)

    releases = []
    for minor in range(12, 0, -1):
        releases.append(f"## 1.{minor}.0\n\n" + w.bullets())
    write(out / "CHANGELOG.md", "# Changelog\n\n" + "\n\n".join(releases))


def many(out, w, count=2000):
    """Lots of small files spread over nested folders."""
    folders = [Path(".")]
    for i in range(20):
        top = Path(f"area-{i:02d}")
        folders.append(top)
        for j in range(w.rng.randint(0, 3)):
            sub = top / f"topic-{j}"
            folders.append(sub)
            if w.rng.random() < 0.3:
                folders.append(sub / "deeper")

    paths = []
    for n in range(count):
        folder = w.rng.choice(folders)
        paths.append(folder / f"note-{n:04d}.md")
    for n, path in enumerate(paths):
        neighbor = w.rng.choice(paths)
        rel = Path(*[".."] * len(path.parent.parts)) / neighbor if path.parent.parts else neighbor
        text = f"# Note {n:04d}\n\n{w.paragraph(1, 3)}\n\n{w.bullets()}\n\nNext: [{neighbor.stem}]({rel.as_posix()})"
        write(out / path, text)

    # Names that tend to break URL handling and sorting.
    for name in ["with space.md", "Über straße.md", "(draft) idea #1.md", "100% done.md", "UPPER.MD", "a.markdown", "ünïcödé/naïve.md"]:
        write(out / "edge-cases" / name, f"# {Path(name).stem}\n\n{w.paragraph()}")
    write(out / "edge-cases" / "not-markdown.txt", "This file should not appear in the sidebar.")

    # Lege skips hidden and .gitignored paths; these must not show up.
    write(out / ".hidden" / "secret.md", "# Hidden\n\nShould not be listed.")
    write(out / "ignored" / "skipped.md", "# Ignored\n\nShould not be listed.")
    (out / ".gitignore").write_text("ignored/\n")


def huge(out, w, megabytes):
    """One very large file mixing every block type, with a table of contents."""
    target = megabytes * 1024 * 1024
    chapters, titles, size = [], [], 0
    while size < target:
        titles.append(f"Chapter {len(titles) + 1}: {w.title()}")
        text = document(w, titles[-1], w.rng.randint(4, 8), level=2)
        chapters.append(text)
        size += len(text.encode())
    n = len(titles)
    # The first 50 chapters, linked by the heading ids comrak generates.
    toc = "\n".join(f"- [{t}](#{t.lower().replace(':', '').replace(' ', '-')})" for t in titles[:50])
    intro = f"# Huge Document\n\nAbout {megabytes} MB across {n} chapters.\n\n## Contents\n\n{toc}"
    write(out / "huge.md", intro + "\n\n" + "\n\n".join(chapters))
    return n


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("out", nargs="?", default="testdata", type=Path)
    parser.add_argument("--huge-mb", type=int, default=10, help="size of huge/huge.md in MB (default 10)")
    parser.add_argument("--many", type=int, default=2000, help="number of files in many/ (default 2000)")
    args = parser.parse_args()

    if args.out.exists():
        shutil.rmtree(args.out)
    baseline(args.out / "baseline", Writer(1))
    many(args.out / "many", Writer(2), args.many)
    chapters = huge(args.out / "huge", Writer(3), args.huge_mb)

    for name in ["baseline", "many", "huge"]:
        files = [p for p in (args.out / name).rglob("*") if p.is_file()]
        size = sum(p.stat().st_size for p in files)
        human = f"{size / 1024 / 1024:.1f} MB" if size >= 1024 * 1024 else f"{size / 1024:.0f} KB"
        print(f"{args.out / name}: {len(files)} files, {human}")
    print(f"huge.md has {chapters} chapters")


if __name__ == "__main__":
    main()
