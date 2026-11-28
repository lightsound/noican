#!/usr/bin/env python3
"""ADR link check: fail if a link between the code and `docs/adr/` is broken.

Policy (2026-11-28): rationale lives in `docs/adr/`, so the links between a
decision record and the code it governs are the only thing that keeps the two
together. Both directions name a target that survives edits and that this
script can resolve:

- Code -> ADR. A pointer in `crates/` or `macos/` reads
  `docs/adr/NNNN-<topic>.md#<section>`. The file must exist and `<section>`
  must be one of its `## ` headings.
- ADR -> code. A `Source:` line in an ADR reads either
  ``Source: `path`.`` (the file as a whole) or ``Source: `path`, `Symbol`.``.
  The path must exist and the symbol must be defined in that file. A symbol
  is the path of definitions leading to it, joined with `::` in Rust and `.`
  in Swift: `Runtime::start`, `tests::servo_holds`, `AppState.startEngine`,
  `FetchError::TooLarge`. A statement inside a function is addressed by the
  function.
- Section headings are unique across all ADRs, so a section name alone
  identifies a record.
- Any other mention of `docs/adr/NNNN-<topic>.md` in `AGENTS.md`, `docs/`
  (the ADRs themselves included), `.agents/`, or `scripts/` must name an
  existing file, and an existing section when it carries a `#<section>`.
- A reference is the repository-root path written as plain text, and it
  is read to the end of its token, so a misspelt file name or section is
  reported as malformed or missing rather than skipped. Inside a code span
  or a quoted string the token runs to the closing mark and is taken
  literally; in running prose it ends at whitespace and may be followed by
  closing punctuation.
- A link into `docs/adr/` is rejected: an inline or reference Markdown
  link, an HTML anchor, or a URL. Whether a rendered link reaches its
  section depends on Markdown's destination grammar (escapes, titles, line
  breaks, autolinks), which this script does not parse, and a relative
  link that works from `docs/` does not contain the root path at all. Name
  the record in plain text instead. Because nothing is parsed, the rule
  also applies inside code fences; an example of link syntax uses the
  `NNNN-<topic>` placeholder, which is never checked.
- A Rust type is defined where its `struct`, `enum`, `union` or `trait`
  is. An `impl` block makes its methods addressable (`Type::method`) but
  does not define `Type`.

Symbols are resolved from indentation and declaration keywords, which rustfmt
and SwiftLint keep regular in this repository; no compiler is involved, so the
check runs anywhere in well under a second. When a definition cannot be
addressed this way, cite the nearest enclosing definition or the file.

Usage:
    python3 scripts/adr-links.py                  # check the repository
    python3 scripts/adr-links.py --symbols PATH   # list the symbols of a file
"""

import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ADR_DIR = "docs/adr"
CODE_DIRS = ("crates", "macos")
CODE_EXTS = (".rs", ".swift")
PROSE_PATHS = ("AGENTS.md", "docs", ".agents", "scripts")
PROSE_EXTS = (".md", ".py", ".sh")
SKIP_DIRS = {".git", "target", ".build"}

ADR_PREFIX = "docs/adr/"
ADR_CLOSERS = {"`": "`", '"': '"', "'": "'"}
ADR_LINK = re.compile(
    r"(?:\]\(\s*<?[^\s<>()]*"
    r"|^ {0,3}\[[^\]\n]+\]:\s*<?\S*"
    r"|\bhref\s*=\s*[\"']?[^\s\"'<>]*"
    r"|://[^\s<>()\"'`]*)"
    r"docs/adr/(?!NNNN-)",
    re.M,
)
ADR_PATH_BREAKS = frozenset(" \t<>()`\"'")
ADR_TRAILING = ".,:;)]>`\"'"
ADR_TARGET = re.compile(r"(\d{4}-[a-z0-9-]+\.md)(?:#([a-z0-9-]+))?")
ADR_PLACEHOLDER = "NNNN-"
SELF = "scripts/adr-links.py"
SOURCE_LINE = re.compile(r"^Source: `([^`]+)`(?:, `([^`]+)`)?\.$")

RUST_DEF = re.compile(
    r"^(?:pub(?:\([^)]*\))?\s+)?"
    r"(?:(?:const|async|unsafe|default|extern(?:\s+\"[^\"]*\")?)\s+)*"
    r"(fn|struct|enum|trait|type|mod|union|const|static|macro_rules!)\s+(?:mut\s+)?"
    r"([A-Za-z_][A-Za-z0-9_]*)"
)
RUST_IMPL = re.compile(
    r"^(?:unsafe\s+)?impl\b(?:<[^{]*?>)?\s+(?:.*?\s+for\s+)?([A-Za-z_][A-Za-z0-9_:]*)"
)
RUST_FIELD = re.compile(r"^(?:pub(?:\([^)]*\))?\s+)?([a-z_][a-z0-9_]*)\s*:")
RUST_VARIANT = re.compile(r"^([A-Z][A-Za-z0-9]*)\s*(?:,|\(|\{|=|$)")

SWIFT_MODIFIERS = (
    r"(?:(?:@\w+(?:\([^)]*\))?|public|private\(set\)|private|fileprivate|internal|open|static|"
    r"final|override|nonisolated(?:\(unsafe\))?|mutating|lazy|weak|unowned|indirect|convenience|"
    r"required|class(?=\s+(?:func|var|let)))\s+)*"
)
SWIFT_DEF = re.compile(
    r"^" + SWIFT_MODIFIERS + r"(func|var|let|struct|class|enum|protocol|extension|actor|typealias)\s+"
    r"([A-Za-z_][A-Za-z0-9_.]*)"
)
SWIFT_INIT = re.compile(r"^" + SWIFT_MODIFIERS + r"(init|deinit)\b")
SWIFT_CASE = re.compile(r"^" + SWIFT_MODIFIERS + r"case\s+(.*)$")
SWIFT_ATTRIBUTE_LINE = re.compile(r"(?:@\w+(?:\([^)]*\))?\s*)+")

RUST_TYPES = {"struct", "enum", "union"}
SWIFT_ENUM_HOSTS = {"enum"}
NAMESPACES = {"struct", "enum", "union", "trait", "mod", "impl", "class", "protocol", "extension", "actor"}
CONTINUATION_STARTS = (")", "]", "}", "{", ".", "where", "->")


def is_swift(path):
    return path.endswith(".swift")


def is_skipped(stripped, swift):
    if not stripped or stripped.startswith("//"):
        return True
    if swift:
        return SWIFT_ATTRIBUTE_LINE.fullmatch(stripped) is not None
    return stripped.startswith("#[") or stripped.startswith("#![")


def swift_case_names(rest):
    depth = 0
    flat = []
    for ch in rest:
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        elif depth == 0:
            flat.append(ch)
    names = []
    for part in "".join(flat).split(","):
        match = re.match(r"\s*([A-Za-z_][A-Za-z0-9_]*)", part.split("=")[0])
        if match:
            names.append(match.group(1))
    return names


def definitions(stripped, swift, host):
    """Return [(kind, name)] defined by this line, given the innermost enclosing kind."""
    inside_body = host is not None and host not in NAMESPACES
    if swift:
        match = SWIFT_INIT.match(stripped)
        if match:
            return [(match.group(1), match.group(1))]
        match = SWIFT_DEF.match(stripped)
        if match:
            kind, name = match.group(1), match.group(2)
            name = name.split(".")[-1] if kind == "extension" else name.split(".")[0]
            if inside_body and kind in {"var", "let"}:
                return []
            return [(kind, name)]
        match = SWIFT_CASE.match(stripped)
        if match and host in SWIFT_ENUM_HOSTS:
            return [("case", name) for name in swift_case_names(match.group(1))]
        return []
    match = RUST_IMPL.match(stripped)
    if match:
        return [("impl", match.group(1).split("::")[-1])]
    match = RUST_DEF.match(stripped)
    if match:
        kind, name = match.group(1), match.group(2)
        if inside_body and kind in {"const", "static"}:
            return []
        return [(kind, name)]
    if host in RUST_TYPES:
        match = RUST_FIELD.match(stripped)
        if match:
            return [("field", match.group(1))]
        match = RUST_VARIANT.match(stripped)
        if match:
            return [("variant", match.group(1))]
    return []


def scan(lines, swift):
    """Resolve every line: return (symbols, owner) where `symbols` is the set of
    defined symbol paths and `owner[i]` the symbol that line i (0-based) defines
    or, failing that, sits inside ("" at file level)."""
    separator = "." if swift else "::"
    stack = []
    symbols = set()
    owner = []
    for line in lines:
        stripped = line.strip()
        if is_skipped(stripped, swift):
            owner.append(None)
            continue
        indent = len(line) - len(line.lstrip(" "))
        if not stripped.startswith(CONTINUATION_STARTS):
            while stack and stack[-1][0] >= indent:
                stack.pop()
        host = stack[-1][1] if stack else None
        scope = separator.join(name for _, _, name in stack)
        defined = definitions(stripped, swift, host)
        for kind, name in defined:
            if kind != "impl":
                symbols.add(separator.join(filter(None, (scope, name))))
        if defined:
            kind, name = defined[0]
            owner.append(separator.join(filter(None, (scope, name))))
            if kind not in {"field", "variant", "case"} or stripped.endswith(("{", "(")):
                stack.append((indent, kind, name))
        else:
            owner.append(scope)
    return symbols, owner


def read_lines(path):
    with open(os.path.join(ROOT, path), encoding="utf-8", errors="replace") as handle:
        return handle.read().split("\n")


def walk(top, extensions):
    full = os.path.join(ROOT, top)
    if os.path.isfile(full):
        yield top
        return
    for directory, subdirectories, files in os.walk(full):
        subdirectories[:] = sorted(d for d in subdirectories if d not in SKIP_DIRS)
        for name in sorted(files):
            if name.endswith(extensions):
                yield os.path.relpath(os.path.join(directory, name), ROOT)


def load_sections(errors):
    sections = {}
    first_seen = {}
    for path in walk(ADR_DIR, (".md",)):
        name = os.path.basename(path)
        sections[name] = set()
        for number, line in enumerate(read_lines(path), 1):
            if not line.startswith("## "):
                continue
            section = line[3:].strip()
            if section in first_seen:
                errors.append(f"{path}:{number}: section `{section}` is already defined in {first_seen[section]}")
            else:
                first_seen[section] = f"{path}:{number}"
            sections[name].add(section)
    return sections


def references(line):
    """Yield what follows `docs/adr/` in every reference on the line.

    A code span or a quoted string that opens right before the path runs to
    its closing mark, and all of it is the reference. Anything else is
    prose, which ends at whitespace and may carry closing punctuation after
    the reference.
    """
    start = line.find(ADR_PREFIX)
    while start != -1:
        opened = start
        while opened > 0 and line[opened - 1] not in ADR_PATH_BREAKS:
            opened -= 1
        closer = ADR_CLOSERS.get(line[opened - 1] if opened > 0 else "")
        begin = end = start + len(ADR_PREFIX)
        while end < len(line) and not line[end].isspace() and line[end] != closer:
            end += 1
        token = line[begin:end]
        yield token if closer else token.rstrip(ADR_TRAILING)
        start = line.find(ADR_PREFIX, end)


def check_links(path, lines, errors):
    """Reject Markdown and HTML links into `docs/adr/` (see the module docstring)."""
    text = "\n".join(lines)
    for link in ADR_LINK.finditer(text):
        number = text.count("\n", 0, link.end()) + 1
        errors.append(
            f"{path}:{number}: link into docs/adr/ (write the reference as plain text, "
            "`docs/adr/NNNN-<topic>.md#<section>`; rendered links are not checked)"
        )


def check_mentions(line, where, sections, errors, require_section):
    """Validate every `docs/adr/...` reference on the line; return how many it holds."""
    found = 0
    for token in references(line):
        if not token or token.startswith(ADR_PLACEHOLDER):
            continue
        found += 1
        match = ADR_TARGET.fullmatch(token)
        if match is None:
            errors.append(
                f"{where}: malformed ADR reference `docs/adr/{token}` "
                "(want docs/adr/NNNN-<topic>.md or docs/adr/NNNN-<topic>.md#<section>)"
            )
            continue
        check_target(match, where, sections, errors, require_section)
    return found


def check_target(match, where, sections, errors, require_section):
    name, section = match.group(1), match.group(2)
    if name not in sections:
        errors.append(f"{where}: docs/adr/{name} does not exist")
    elif section is None:
        if require_section:
            errors.append(f"{where}: pointer to docs/adr/{name} names no section (append #<section>)")
    elif section not in sections[name]:
        errors.append(f"{where}: docs/adr/{name} has no section `{section}`")


def check_sources(errors):
    cache = {}
    checked = 0
    for path in walk(ADR_DIR, (".md",)):
        for number, line in enumerate(read_lines(path), 1):
            if not line.startswith("Source:"):
                continue
            where = f"{path}:{number}"
            match = SOURCE_LINE.match(line)
            if not match:
                errors.append(f"{where}: malformed Source line (want ``Source: `path`.`` or ``Source: `path`, `Symbol`.``)")
                continue
            checked += 1
            target, symbol = match.group(1), match.group(2)
            if not os.path.isfile(os.path.join(ROOT, target)):
                errors.append(f"{where}: {target} does not exist")
                continue
            if symbol is None:
                continue
            if target not in cache:
                cache[target] = scan(read_lines(target), is_swift(target))[0]
            if symbol not in cache[target]:
                errors.append(f"{where}: `{symbol}` is not defined in {target}")
    return checked


def check_pointers(sections, errors):
    checked = 0
    for top in CODE_DIRS:
        for path in walk(top, CODE_EXTS):
            lines = read_lines(path)
            check_links(path, lines, errors)
            for number, line in enumerate(lines, 1):
                checked += check_mentions(line, f"{path}:{number}", sections, errors, require_section=True)
    return checked


def check_prose(sections, errors):
    for top in PROSE_PATHS:
        for path in walk(top, PROSE_EXTS):
            if path == SELF:
                continue
            lines = read_lines(path)
            check_links(path, lines, errors)
            for number, line in enumerate(lines, 1):
                check_mentions(line, f"{path}:{number}", sections, errors, require_section=False)


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "--symbols":
        for symbol in sorted(scan(read_lines(sys.argv[2]), is_swift(sys.argv[2]))[0]):
            print(symbol)
        return 0
    if len(sys.argv) != 1:
        print(__doc__)
        return 2
    errors = []
    sections = load_sections(errors)
    sources = check_sources(errors)
    pointers = check_pointers(sections, errors)
    check_prose(sections, errors)
    if sources == 0 or pointers == 0:
        errors.append(f"found {sources} Source lines and {pointers} code pointers; the scan itself is broken")
    if errors:
        print("ADR link check FAILED:\n")
        for error in errors:
            print(f"  {error}")
        print(
            "\nFix the link, not the check: point at the current file, symbol, or"
            "\nsection, or rewrite the record if the decision itself changed."
            "\n`python3 scripts/adr-links.py --symbols <file>` lists what a file defines."
        )
        return 1
    count = sum(len(names) for names in sections.values())
    print(f"ADR link check OK: {sources} Source lines, {pointers} code pointers, {count} sections.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
