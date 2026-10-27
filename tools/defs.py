"""defs.py - the one Python reader of the IPMC build configuration, crates/ipmc-defs/src/defs.rs.

`read(path=None)` returns {NAME: value} for every `pub const NAME: T = value;` item of the file
(default: crates/ipmc-defs/src/defs.rs). A value is an int (decimal, 0x hex, `_` separators
allowed, a leading minus), a bool, a str (a "text" literal), a bare earlier NAME (resolved) or
an enum variant (`Type::Variant`, returned as the variant name string). Everything else in the
file is a comment, an attribute, an enum body or a blank line. rustfmt wraps a long value onto
the next line (`pub const NAME: T =` then the value indented); that form, and only that, is
joined. The six FRU strings that name the Tier exist in the file as `NAME_T1`, `NAME_T2` and
`NAME_T3`; `read` also returns `NAME` itself, the variant of `tier` (default 1, the public
build; ipmc-defs lib.rs selects the same way from IPMC_TIER). Any line this reader does not understand raises DefsError with the file and line number.

Command line:  python -I tools/defs.py NAME [NAME ...]   prints one value per line.
"""

import re
import sys
from pathlib import Path

DEFAULT_PATH = Path(__file__).resolve().parents[1] / "crates" / "ipmc-defs" / "src" / "defs.rs"

_CONST = re.compile(r"pub const (\w+): (&str|bool|[ui](?:8|16|32|64)|usize|\w+) = (.+);$")
_INT = re.compile(r"-?(?:0x[0-9A-Fa-f_]+|[0-9][0-9_]*)$")
_STRING = re.compile(r'"([ -!#-\[\]-~]*)"$')  # printable ASCII, no quote, no backslash
_VARIANT = re.compile(r"(\w+)::(\w+)$")
_NAME = re.compile(r"[A-Z][A-Z0-9_]*$")
_WRAPPED = re.compile(r"pub const \w+: (?:&str|bool|[ui](?:8|16|32|64)|usize|\w+) =$")
_ENUM_OPEN = re.compile(r"pub enum \w+ \{$")
_VARIANT_LINE = re.compile(r"\w+,$")


class DefsError(ValueError):
    """A line of the defs file this reader does not understand."""


def _value(text, type_name, known, where):
    if type_name == "&str":
        found = _STRING.match(text)
        if not found:
            raise DefsError(f"{where}: not a plain string literal: {text}")
        return found.group(1)
    if type_name == "bool":
        if text not in ("true", "false"):
            raise DefsError(f"{where}: not true or false: {text}")
        return text == "true"
    if re.fullmatch(r"[ui](?:8|16|32|64)|usize", type_name):
        if _INT.match(text):
            return int(text.replace("_", ""), 0)
        if _NAME.match(text) and text in known and isinstance(known[text], int):
            return known[text]
        raise DefsError(f"{where}: not an integer or an earlier name: {text}")
    variant = _VARIANT.match(text)  # any other type name is an enum
    if variant and variant.group(1) == type_name:
        return variant.group(2)
    raise DefsError(f"{where}: not a {type_name} variant: {text}")


TIER_NAMES = (
    "IPMC_FRU_BOARD_PRODUCT",
    "IPMC_FRU_BOARD_SERIAL",
    "IPMC_FRU_BOARD_PART",
    "IPMC_FRU_PRODUCT_NAME",
    "IPMC_FRU_PRODUCT_PART",
    "IPMC_FRU_PRODUCT_SERIAL",
)


def read(path=None, tier=1):
    """{NAME: value} of the defs file at `path` (default: the repository's defs.rs); the six
    TIER_NAMES are added as the `_T<tier>` variant, 1..=3."""
    if tier not in (1, 2, 3):
        raise DefsError(f"tier must be 1, 2 or 3, not {tier}")
    path = Path(path) if path else DEFAULT_PATH
    values = {}
    in_enum = False
    lines = path.read_text(encoding="utf-8").splitlines()
    skip = False
    for number, raw in enumerate(lines, 1):
        if skip:
            skip = False
            continue
        where = f"{path}:{number}"
        line = raw.strip()
        if _WRAPPED.match(line):
            if number >= len(lines) or not lines[number].startswith("    "):
                raise DefsError(f"{where}: a wrapped value must follow, indented: {line}")
            line = f"{line} {lines[number].strip()}"
            skip = True
        if not line or line.startswith("//") or line.startswith("#["):
            continue
        if in_enum:
            if line == "}":
                in_enum = False
            elif not _VARIANT_LINE.match(line):
                raise DefsError(f"{where}: not an enum variant: {line}")
            continue
        if _ENUM_OPEN.match(line):
            in_enum = True
            continue
        const = _CONST.match(line)
        if not const:
            raise DefsError(f"{where}: not a pub const item: {line}")
        name, type_name, text = const.groups()
        if name in values:
            raise DefsError(f"{where}: {name} defined twice")
        values[name] = _value(text.strip(), type_name, values, where)
    if in_enum:
        raise DefsError(f"{path}: enum not closed")
    for name in TIER_NAMES:
        if f"{name}_T{tier}" in values:
            values[name] = values[f"{name}_T{tier}"]
    return values


def main(argv):
    values = read()
    for name in argv:
        if name not in values:
            sys.exit(f"defs.py: {name} is not in {DEFAULT_PATH}")
        print(values[name])
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
