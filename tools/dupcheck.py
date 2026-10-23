"""dupcheck.py - duplicate-function finder for the Rust crates (owner directive 10-23-2026).

Usage:
    python -I tools/dupcheck.py                scan crates/ and gate
    python -I tools/dupcheck.py --self-test    prove the scanner on a built-in fixture
    python -I tools/dupcheck.py --root DIR     scan another tree
    python -I tools/dupcheck.py --test-ratio R explore with a lower test threshold (not the gate)

What it does
    Every `.rs` file under crates/ (skipping target/) is lexed as Rust (string, raw string, byte string,
    char and lifetime tokens, nested block comments) with comments dropped; identifiers and literals
    are normalised (`I` and `L`), so a renamed copy of a function has the same token sequence. Every
    `fn` body is extracted. Pairs are scored by 5-token shingle Jaccard, then by the
    difflib ratio of the token sequences. Judgement is by what the code does, not by its name.

Production and test code are separate classes
    Test code is a file under a `tests/` directory or the body of a `#[cfg(test)]` module. A pair is
    only compared inside one class.
    - Production: FAIL at ratio >= 0.85 with >= 30 tokens in both bodies.
    - Test:       FAIL at ratio >= 0.92 with >= 25 tokens in both bodies.
    Exit status 1 when any pair fails, 0 otherwise.

Exclusion by rule (no allowlist)
    A body that is only a mapping table is skipped. The body is exactly one `match` (the whole body)
    whose scrutinee is `self`, `*self` or one plain identifier, with no guards, and whose every arm is
    `Pattern => literal` or `Pattern => path`. A path is identifiers joined by `::`, optionally a
    unit/tuple constructor whose arguments are literals or paths (`Some(1)`, `E::A(x, 2)`). `|`
    alternatives and `_` are allowed. A call, method call, `?`, block, guard, field access or
    arithmetic in the scrutinee or an arm disqualifies the body. Each enum's table is its own data,
    not a duplicate.

Block warning (never fails)
    Runs of >= 80 identical normalised tokens shared by two different bodies are printed as WARNING
    lines, to catch a duplicated block inside larger functions. The bodies are function bodies,
    `macro_rules!` bodies, `const`/`static` initialiser expressions (also associated consts in `impl`
    blocks) that sit outside functions. Declaration filter: a run between function bodies is reported
    only when it contains a statement keyword (`let if else match for while loop return`); a run that
    involves a macro or initialiser block is reported when it contains a statement keyword or a call
    (an identifier directly before `(`). A run of consts, statics, enum variants, struct fields or
    table rows has neither and is skipped. Pairs that already FAIL are not repeated.
"""
import difflib
import os
import re
import sys

sys.stdout.reconfigure(encoding="utf-8")

PROD_RATIO, PROD_TOK = 0.85, 30
TEST_RATIO, TEST_TOK = 0.92, 25
MIN_BODY = min(PROD_TOK, TEST_TOK)
BLOCK_RUN = 80
SHINGLE = 5
STATEMENT_KW = {"let", "if", "else", "match", "for", "while", "loop", "return"}

PUNCT = re.compile(r"::|->|=>|==|!=|<=|>=|&&|\|\||<<|>>|\.\.=|\.\.|\S")
WORD = re.compile(r"[A-Za-z_]\w*")
NUMBER = re.compile(r"\d\w*")
RAW_OPEN = re.compile(r'(?:br|cr|r)(#*)"')
KEYWORDS = set(
    """as break const continue else enum fn for if impl in let loop match mut pub ref return self
Self static struct trait true false type unsafe use where while dyn Some None Ok Err""".split()
)


def _skip_quoted(src, i, quote):
    """Index just past the quoted literal whose opening quote is at src[i]."""
    n = len(src)
    i += 1
    while i < n:
        if src[i] == "\\":
            i += 2
        elif src[i] == quote:
            return i + 1
        else:
            i += 1
    return n


def lex(src):
    """Lex Rust source. Return [(kind, text, line)]; kind is 'w' word, 'n' number, 's' string or char
    literal, 'l' lifetime, 'p' punctuation. Comments (line and nested block) are dropped."""
    out, i, n, line = [], 0, len(src), 1
    while i < n:
        c = src[i]
        if c == "\n":
            line += 1
            i += 1
        elif c.isspace():
            i += 1
        elif src.startswith("//", i):
            while i < n and src[i] != "\n":
                i += 1
        elif src.startswith("/*", i):
            depth = 0
            while i < n:
                if src.startswith("/*", i):
                    depth += 1
                    i += 2
                elif src.startswith("*/", i):
                    depth -= 1
                    i += 2
                    if depth == 0:
                        break
                else:
                    if src[i] == "\n":
                        line += 1
                    i += 1
        else:
            start = i
            m = RAW_OPEN.match(src, i)
            if m:  # raw string: r"..", r#".."#, br#".."#, cr#".."#
                end = src.find('"' + m.group(1), m.end())
                i = n if end < 0 else end + 1 + len(m.group(1))
                kind = "s"
            elif c == '"' or (c in "bc" and src.startswith('"', i + 1)):  # string, b"..", c".."
                i = _skip_quoted(src, i + (c != '"'), '"')
                kind = "s"
            elif c == "b" and src.startswith("'", i + 1):  # byte literal b'x'
                i = _skip_quoted(src, i + 1, "'")
                kind = "s"
            elif c == "'":
                if src.startswith("\\", i + 1) or src.startswith("'", i + 2):  # char literal
                    i = _skip_quoted(src, i, "'")
                    kind = "s"
                else:  # lifetime or label
                    i += 1
                    w = WORD.match(src, i)
                    i = w.end() if w else i
                    kind = "l"
            elif WORD.match(src, i):
                i = WORD.match(src, i).end()
                kind = "w"
            elif c.isdigit():
                i = NUMBER.match(src, i).end()
                kind = "n"
            else:
                i = PUNCT.match(src, i).end()
                kind = "p"
            text = src[start:i]
            out.append((kind, text, line))
            line += text.count("\n")
    return out


def tokenise(src):
    """Return [(normalised, line, raw)]."""
    out = []
    for kind, t, line in lex(src):
        if kind == "w":
            n = t if t in KEYWORDS else "I"
        elif kind in "ns":
            n = "L"
        elif kind == "l":
            n = "LT"
        else:
            n = t
        out.append((n, line, t))
    return out


def match_close(ts, j, open_t="{", close_t="}"):
    """Index of the token closing the bracket opened at ts[j]."""
    depth = 0
    for k in range(j, len(ts)):
        if ts[k][2] == open_t:
            depth += 1
        elif ts[k][2] == close_t:
            depth -= 1
            if depth == 0:
                return k
    return len(ts) - 1


def test_ranges(ts):
    """Token index ranges of every `#[cfg(test)] mod name { .. }`."""
    rng = []
    for i in range(len(ts) - 5):
        if ts[i][2] == "#" and ts[i + 1][2] == "[" and ts[i + 2][2] == "cfg" and ts[i + 4][2] == "test":
            j = i
            while j < len(ts) and ts[j][2] != "{" and ts[j][2] != ";":
                j += 1
            if j < len(ts) and ts[j][2] == "{":
                rng.append((j, match_close(ts, j)))
    return rng


def _split_top(raw, a, b):
    """Split raw[a:b] at top-level commas; return [(start, end)]."""
    parts, depth, start = [], 0, a
    for k in range(a, b):
        if raw[k] in ("(", "[", "{"):
            depth += 1
        elif raw[k] in (")", "]", "}"):
            depth -= 1
        elif raw[k] == "," and depth == 0:
            parts.append((start, k))
            start = k + 1
    if start < b:
        parts.append((start, b))
    return parts


def _is_value(n, raw, a, b):
    """True when raw[a:b] is a literal, a path, or a unit/tuple constructor of values."""
    if a >= b:
        return False
    if b - a == 1 and n[a] in ("L", "true", "false"):
        return True
    if b - a == 2 and raw[a] == "-" and n[a + 1] == "L":
        return True
    k = a
    if n[k] not in ("I", "Self", "None", "Some", "Ok", "Err"):
        return False
    k += 1
    while k + 1 < b and raw[k] == "::" and n[k + 1] in ("I", "Self", "None"):
        k += 2
    if k == b:
        return n[a] not in ("Some", "Ok", "Err")
    if not raw[k - 1][:1].isupper() or raw[k] != "(" or raw[b - 1] != ")" or match_close(_Slice(raw), k, "(", ")") != b - 1:
        return False
    args = _split_top(raw, k + 1, b - 1)
    return bool(args) and all(_is_value(n, raw, x, y) for x, y in args)


class _Slice:
    """Adapter so match_close can walk a list of raw strings."""

    def __init__(self, raw):
        self.raw = raw

    def __len__(self):
        return len(self.raw)

    def __getitem__(self, k):
        return (None, None, self.raw[k])


def is_mapping_table(body):
    """True for a body that is exactly one guard-free `match` on `self`, `*self` or a plain identifier
    whose every arm is `Pattern => literal | path` (see the module header)."""
    n = [t[0] for t in body]
    raw = [t[2] for t in body]
    if len(n) < 6 or raw[0] != "{" or raw[1] != "match" or raw[-1] != "}":
        return False
    j = 2
    while j < len(n) and raw[j] != "{":
        j += 1
    scrut = raw[2:j]
    if scrut not in (["self"], ["*", "self"]) and not (len(scrut) == 1 and n[2] == "I"):
        return False
    if j >= len(n) or match_close(body, j) != len(n) - 2:  # match must be the whole body
        return False
    arms = _split_top(raw, j + 1, len(n) - 2)
    if not arms:
        return False
    for a, b in arms:
        depth, arrow = 0, None
        for k in range(a, b):
            if raw[k] in ("(", "[", "{"):
                depth += 1
            elif raw[k] in (")", "]", "}"):
                depth -= 1
            elif raw[k] == "if" and depth == 0 and arrow is None:
                return False  # guard
            elif raw[k] == "=>" and depth == 0:
                arrow = k
                break
        if arrow is None or arrow == a or not _is_value(n, raw, arrow + 1, b):
            return False
    return True


class Fn:
    def __init__(self, path, line, name, body, test, block=False):
        self.path, self.line, self.name, self.test, self.block = path, line, name, test, block
        self.toks = [t[0] for t in body]
        self.sh = set(tuple(self.toks[x:x + SHINGLE]) for x in range(len(self.toks) - SHINGLE + 1))

    def where(self):
        return "%s:%d %s" % (self.path, self.line, self.name)


def _has_call(seg):
    return any(seg[k] == "I" and seg[k + 1] == "(" for k in range(len(seg) - 1))


def _blocks(ts, fn_spans, ranges, path, force_test):
    """Non-function blocks: macro_rules! bodies and const/static initialisers outside any fn body."""
    out = []
    for i in range(len(ts)):
        if any(a <= i <= b for a, b in fn_spans):
            continue
        first, last, name = None, None, None
        if ts[i][2] == "macro_rules" and i + 3 < len(ts) and ts[i + 1][2] == "!":
            name = ts[i + 2][2]
            if ts[i + 3][2] in ("{", "(", "["):
                close = {"{": "}", "(": ")", "[": "]"}[ts[i + 3][2]]
                first, last = i + 3, match_close(ts, i + 3, ts[i + 3][2], close)
        elif ts[i][2] in ("const", "static") and i + 1 < len(ts) and ts[i + 1][2] not in ("fn", "unsafe", "{"):
            k = i
            while k < len(ts) and ts[k][2] not in ("=", ";", "{"):
                k += 1
            if k < len(ts) and ts[k][2] == "=":
                name, first, depth = ts[i + 1][2], k + 1, 0
                last = first
                while last < len(ts):
                    if ts[last][2] in ("(", "[", "{"):
                        depth += 1
                    elif ts[last][2] in (")", "]", "}"):
                        depth -= 1
                    elif ts[last][2] == ";" and depth == 0:
                        break
                    last += 1
        if first is not None and last - first + 1 >= BLOCK_RUN:
            test = force_test or any(a < i < b for a, b in ranges)
            out.append(Fn(path, ts[i][1], name, ts[first:last + 1], test, block=True))
    return out


def extract(path, src, force_test=False):
    """Return (non-table fns with >= MIN_BODY tokens, count of table bodies skipped, non-fn blocks)."""
    ts = tokenise(src)
    ranges = test_ranges(ts)
    fns, spans, skipped, i = [], [], 0, 0
    while i < len(ts):
        if ts[i][2] == "fn" and i + 1 < len(ts) and ts[i + 1][0] == "I":
            name, line = ts[i + 1][2], ts[i][1]
            j = i
            while j < len(ts) and ts[j][2] not in ("{", ";"):
                j += 1
            if j < len(ts) and ts[j][2] == "{":
                k = match_close(ts, j)
                body = ts[j:k + 1]
                test = force_test or any(a < i < b for a, b in ranges)
                if is_mapping_table(body):
                    skipped += 1
                elif len(body) >= MIN_BODY:
                    fns.append(Fn(path, line, name, body, test))
                spans.append((j, k))
                i = j + 1
                continue
        i += 1
    return fns, skipped, _blocks(ts, spans, ranges, path, force_test)


def score_pairs(fns):
    """Yield (ratio, a, b, ntok) for every same-class pair at or above its class threshold."""
    for a in range(len(fns)):
        fa = fns[a]
        for b in range(a + 1, len(fns)):
            fb = fns[b]
            if fa.test != fb.test:
                continue
            ratio_min, tok_min = (TEST_RATIO, TEST_TOK) if fa.test else (PROD_RATIO, PROD_TOK)
            la, lb = len(fa.toks), len(fb.toks)
            if min(la, lb) < tok_min or min(la, lb) / max(la, lb) < 0.6:
                continue
            inter = len(fa.sh & fb.sh)
            if not inter or inter / len(fa.sh | fb.sh) < 0.45:
                continue
            r = difflib.SequenceMatcher(None, fa.toks, fb.toks, autojunk=False).ratio()
            if r >= ratio_min:
                yield r, fa, fb, min(la, lb)


def block_warnings(fns, failing):
    """Runs of >= BLOCK_RUN identical normalised tokens shared by two different bodies."""
    index = {}
    for fi, f in enumerate(fns):
        for p in range(len(f.toks) - BLOCK_RUN + 1):
            index.setdefault(tuple(f.toks[p:p + BLOCK_RUN]), []).append((fi, p))
    diag = {}  # (fa, fb, pa - pb) -> sorted list of pa
    for hits in index.values():
        for x in range(len(hits)):
            for y in range(x + 1, len(hits)):
                (fa, pa), (fb, pb) = hits[x], hits[y]
                if fa == fb or fns[fa].test != fns[fb].test:
                    continue
                if (fns[fa], fns[fb]) in failing or (fns[fb], fns[fa]) in failing:
                    continue
                diag.setdefault((fa, fb, pa - pb), []).append(pa)
    out = []
    for (fa, fb, _), starts in diag.items():
        starts.sort()
        run_start = prev = starts[0]
        for s in starts[1:] + [None]:
            if s is not None and s == prev + 1:
                prev = s
                continue
            length = prev - run_start + BLOCK_RUN
            seg = fns[fa].toks[run_start:run_start + length]
            if STATEMENT_KW & set(seg) or ((fns[fa].block or fns[fb].block) and _has_call(seg)):
                out.append((length, fns[fa], fns[fb]))
            if s is not None:
                run_start = prev = s
    return sorted(out, key=lambda x: -x[0])


def run(fns, skipped, label, blocks=()):
    pairs = sorted(score_pairs(fns), key=lambda x: -x[0])
    failing = set((a, b) for _, a, b, _ in pairs)
    print("functions scanned: %d (>= %d tokens), non-function blocks: %d, mapping tables skipped by rule: %d"
          % (len(fns), MIN_BODY, len(blocks), skipped))
    for title, want in (("PRODUCTION", False), ("TESTS", True)):
        rows = [p for p in pairs if p[1].test == want]
        thr = (TEST_RATIO, TEST_TOK) if want else (PROD_RATIO, PROD_TOK)
        print("\n== %s pairs (ratio >= %.2f, >= %d tokens): %d ==" % (title, thr[0], thr[1], len(rows)))
        for r, a, b, n in rows:
            print("FAIL %.2f %4d  %s  <->  %s" % (r, n, a.where(), b.where()))
    warns = block_warnings(list(fns) + list(blocks), failing)
    print("\n== BLOCK warnings (>= %d identical tokens, statements present): %d ==" % (BLOCK_RUN, len(warns)))
    for length, a, b in warns:
        print("WARN %4d tokens  %s  <->  %s" % (length, a.where(), b.where()))
    print("\n%s: %d failing pair(s), %d warning(s)" % (label, len(pairs), len(warns)))
    return 1 if pairs else 0


def scan(root):
    fns, skipped, blocks = [], 0, []
    for d, dirs, files in os.walk(root):
        dirs[:] = [x for x in dirs if x != "target"]
        for f in sorted(files):
            if f.endswith(".rs"):
                path = os.path.join(d, f)
                with open(path, encoding="utf-8") as fh:
                    src = fh.read()
                rel = os.path.relpath(path, os.path.dirname(root) if os.path.basename(root) == "crates" else root)
                got, sk, blk = extract(rel.replace(os.sep, "/"), src, os.sep + "tests" + os.sep in path)
                fns += got
                blocks += blk
                skipped += sk
    return fns, skipped, blocks


FIXTURE = """
pub fn copy_one(buf: &mut [u8], n: usize) -> u8 {
    let mut sum = 0u8;
    let mut i = 0;
    while i < n { sum = sum.wrapping_add(buf[i]); buf[i] = sum ^ 0x55; i += 1; }
    if sum == 0 { return 1; }
    sum.wrapping_sub(buf[0]).wrapping_add(7)
}
pub fn copy_two(data: &mut [u8], count: usize) -> u8 {
    let mut total = 0u8;
    let mut k = 0;
    while k < count { total = total.wrapping_add(data[k]); data[k] = total ^ 0x99; k += 1; }
    if total == 0 { return 2; }
    total.wrapping_sub(data[0]).wrapping_add(9)
}
impl A { pub fn name(self) -> &'static str { match self {
    A::One => "one", A::Two => "two", A::Three => "three", A::Four => "four", A::Five => "five" } } }
impl B { pub fn text(&self) -> &'static str { match *self {
    B::Red => "red", B::Green => "green", B::Blue => "blue", B::Cyan => "cyan", B::Pink => "pink" } } }
"""


URL_FIXTURE = """
/* header /* nested */ still comment fn ghost() {} */
pub fn one(buf: &mut [u8], n: usize) -> usize {
    let url = "https://x"; // trailing comment
    let raw = r#"a "quoted" // not a comment /* nor this"#;
    let mut i = 0;
    while i < n { buf[i] = buf[i].wrapping_add(url.len() as u8); i += raw.len(); }
    if i > n { return 1; }
    i + 3
}
pub fn two<'a>(data: &'a mut [u8], count: usize) -> usize {
    let link = "https://x"; /* trailing */
    let text = r#"a "quoted" // not a comment /* nor this"#;
    let mut k = b'\'; let _c = '"';
    k = 0;
    while k < count { data[k] = data[k].wrapping_add(link.len() as u8); k += text.len(); }
    if k > count { return 2; }
    k + 5
}
"""

GUARD_FIXTURE = "{ match dangerous_read() { A if predicate() => do_io(), B => fail() } }"
GUARD2_FIXTURE = "{ match self { A if ok() => 1, B => 2, C => 3 } }"
CALL_FIXTURE = "{ match self { A => f(), B => 2, C => 3 } }"
TABLE_FIXTURE = "{ match self { A::X | A::Y => Some(1), A::Z(Q) => None, _ => B::W(2, C::D) } }"

MACRO_FIXTURE = "macro_rules! %s { ($x:expr) => { %s } }"


def self_test():
    fns, skipped, _ = extract("fixture.rs", FIXTURE)
    names = sorted(f.name for f in fns)
    pairs = list(score_pairs(fns))
    ok = (
        names == ["copy_one", "copy_two"]
        and skipped == 2
        and len(pairs) == 1
        and {pairs[0][1].name, pairs[0][2].name} == {"copy_one", "copy_two"}
    )
    print("self-test: %d renamed-copy pair flagged, %d enum tables skipped: %s" % (len(pairs), skipped, "PASS" if ok else "FAIL"))

    ufns, _, _ = extract("url.rs", URL_FIXTURE)
    upairs = list(score_pairs(ufns))
    uok = sorted(f.name for f in ufns) == ["one", "two"] and len(upairs) == 1
    print("self-test: strings, raw strings, char/lifetime and nested comments: %d fns, %d pair flagged: %s"
          % (len(ufns), len(upairs), "PASS" if uok else "FAIL"))

    def tbl(src):
        return is_mapping_table(tokenise(src))

    gok = (not tbl(GUARD_FIXTURE) and not tbl(GUARD2_FIXTURE) and not tbl(CALL_FIXTURE) and tbl(TABLE_FIXTURE))
    print("self-test: guarded/calling match not exempt, plain table exempt: %s" % ("PASS" if gok else "FAIL"))

    stmts = "let a = f(1); let b = g(a); if a > b { return a; } " * 6
    blk = [extract("m%d.rs" % k, MACRO_FIXTURE % ("mac%d" % k, stmts))[2] for k in (1, 2)]
    consts = "const X: u8 = %s;" % ("f(1) + " * 40 + "2")
    cblk = [extract("c%d.rs" % k, consts)[2] for k in (1, 2)]
    allb = blk[0] + blk[1]
    callb = cblk[0] + cblk[1]
    bok = len(allb) == 2 and len(callb) == 2 and block_warnings(allb, set()) != [] and block_warnings(callb, set()) != []
    print("self-test: duplicated macro body and const initialiser warned: %s" % ("PASS" if bok else "FAIL"))
    return 0 if (ok and uok and gok and bok) else 1


def main(argv):
    if "--self-test" in argv:
        return self_test()
    global TEST_RATIO
    root = "crates"
    if "--root" in argv:
        root = argv[argv.index("--root") + 1]
    if "--test-ratio" in argv:  # exploration only: look for weaker test duplicates
        TEST_RATIO = float(argv[argv.index("--test-ratio") + 1])
    fns, skipped, blocks = scan(root)
    return run(fns, skipped, "dupcheck", blocks)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
