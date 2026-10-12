#!/usr/bin/env python3
"""Check the HTML docs: tags balance, internal links and anchors resolve, and
no page loads anything from the network. Exits non-zero on any problem."""
import sys
from html.parser import HTMLParser
from pathlib import Path

DOCS = Path(__file__).resolve().parent.parent / "docs"
VOID = {"meta", "br", "hr", "img", "link", "input"}


class Page(HTMLParser):
    def __init__(self):
        super().__init__()
        self.stack, self.ids, self.links, self.problems = [], set(), [], []

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if "id" in attrs:
            self.ids.add(attrs["id"])
        if tag == "a" and "href" in attrs:
            self.links.append(attrs["href"])
        for key in ("src", "href"):
            value = attrs.get(key) or ""
            if tag != "a" and value.startswith(("http:", "https:", "//")):
                self.problems.append(f"loads an external resource: {value}")
        if tag in ("script", "iframe"):
            self.problems.append(f"<{tag}> is not allowed")
        if tag not in VOID:
            self.stack.append(tag)

    def handle_endtag(self, tag):
        if self.stack and self.stack[-1] == tag:
            self.stack.pop()
        else:
            self.problems.append(f"unexpected </{tag}>")


def main():
    pages = {}
    for path in sorted(DOCS.rglob("*.html")):
        page = Page()
        page.feed(path.read_text(encoding="utf-8"))
        if page.stack:
            page.problems.append(f"unclosed tags: {page.stack}")
        pages[path] = page
    failed = False
    for path, page in pages.items():
        for link in page.links:
            if link.startswith(("http:", "https:", "mailto:")):
                continue
            target, _, anchor = link.partition("#")
            dest = (path.parent / target).resolve() if target else path
            if dest not in pages:
                page.problems.append(f"broken link: {link}")
            elif anchor and anchor not in pages[dest].ids:
                page.problems.append(f"broken anchor: {link}")
        for problem in page.problems:
            failed = True
            print(f"{path.relative_to(DOCS.parent)}: {problem}")
    print(f"docs: {len(pages)} pages checked, {'FAILED' if failed else 'ok'}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
