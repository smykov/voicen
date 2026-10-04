# T-035 source scan for scripts/ci/shell-test-layout.sh (docs/decisions/ci-toolchain.md).
# Input: the shell's *.rs files. Output: one line per finding, "FILE:LINE: TEXT  (NOTE)";
# no output means no finding. POSIX awk (gawk, mawk, busybox); run with LC_ALL=C.
#
# Pass 1 (lex) reads a file the way rustc tokenizes it, as far as the checks need: every
# comment is dropped, doc comments (/// and //! lines, /** */ and /*! */ blocks) and nested
# block comments included; string, raw-string and char literals become placeholders. So
# text inside a comment or a literal never counts as code (T-038, decision #41: doc text is
# not read at all; the doctest switches are pinned by shell-test-layout.sh instead).
# Pass 2 (scan) finds every attribute `#[...]` / `#![...]` anywhere on a line, with any
# whitespace or line breaks inside it, and the token `include`:
#   attribute: test attribute (name `test*`, any path) or a cfg/cfg_attr predicate naming
#              `test`; cfg_attr's attribute arguments are checked the same way;
#              `path = ...` (a #[path] module: a file outside the scanned set);
#   code:      the token `include` anywhere (include!, also renamed by `use` or written
#              r#include: a file outside the scanned set; identifiers that only contain
#              it, include_str! and include_bytes! pass).
# Text the lexer cannot close (block comment, string, attribute) is a finding too.

BEGIN { SO = "\001"; SE = "\002"; nstr = 0 }

FNR == 1 && NR > 1 { scan_file(cur) }
FNR == 1 { cur = FILENAME; m = 0 }
{ sub(/\r$/, ""); L[++m] = $0 }
END { if (NR > 0) scan_file(cur) }

function report(f, ln, text, note) {
  gsub(/^[ \t]+|[ \t]+$/, "", text)
  if (length(text) > 120) text = substr(text, 1, 117) "..."
  printf "%s:%d: %s  (%s)\n", f, ln, text, note
}

# A literal's placeholder index; its content is not kept (no check reads it).
function newstr() {
  return ++nstr
}

function scan_file(f) {
  nstr = 0
  lex(f)
  scan(f)
}

# ---- pass 1 -------------------------------------------------------------------------
# C[k]: code of source line k; comments (doc comments included) removed, literals replaced
# by SO idx SE.
function lex(f,    k, ln, n, i, c, nx, o, j, h, p, pp, mode, bdepth, bline, sidx, sline, rterm, t) {
  mode = 0  # 0 code, 1 block comment, 2 string, 3 raw string
  for (k = 1; k <= m; k++) {
    ln = L[k]; n = length(ln); i = 1; o = ""
    while (i <= n) {
      if (mode == 1) {
        t = substr(ln, i, 2)
        if (t == "/*") { bdepth++; i += 2; continue }
        if (t == "*/") {
          bdepth--
          if (bdepth == 0) mode = 0
          i += 2; continue
        }
        i++; continue
      }
      if (mode == 2) {
        c = substr(ln, i, 1)
        if (c == "\\") { i += 2; continue }
        if (c == "\"") { mode = 0; i++; continue }
        i++; continue
      }
      if (mode == 3) {
        if (substr(ln, i, length(rterm)) == rterm) { mode = 0; i += length(rterm); continue }
        i++; continue
      }
      c = substr(ln, i, 1)
      if (c == "/") {
        nx = substr(ln, i + 1, 1)
        # A line comment, doc or not (///, //!): the rest of the line is dropped.
        if (nx == "/") { o = o " "; i = n + 1; continue }
        # A block comment, doc or not (/** */, /*! */): dropped up to its matching */.
        if (nx == "*") { mode = 1; bdepth = 1; bline = k; o = o " "; i += 2; continue }
      }
      if (c == "\"") {
        sidx = newstr(); sline = k; o = o SO sidx SE; mode = 2; i++; continue
      }
      if (c == "r") {
        p = substr(ln, i - 1, 1); pp = substr(ln, i - 2, 1)
        if (p !~ /[A-Za-z0-9_]/ || (p ~ /[bc]/ && pp !~ /[A-Za-z0-9_]/)) {
          j = i + 1; h = ""
          while (substr(ln, j, 1) == "#") { h = h "#"; j++ }
          if (substr(ln, j, 1) == "\"") {
            sidx = newstr(); sline = k; o = o SO sidx SE
            rterm = "\"" h; mode = 3; i = j + 1; continue
          }
        }
      }
      if (c == "'") {
        nx = substr(ln, i + 1, 1)
        if (nx == "\\") {
          j = index(substr(ln, i + 3), "'")
          if (j > 0) { o = o "''"; i = i + 3 + j; continue }
        } else if (nx ~ /[A-Za-z0-9_]/) {
          if (substr(ln, i + 2, 1) == "'") { o = o "''"; i += 3; continue }
          # otherwise a lifetime or label
        } else if (nx != "") {
          j = index(substr(ln, i + 1, 5), "'")
          if (j > 0) { o = o "''"; i = i + j + 1; continue }
        }
      }
      o = o c; i++
    }
    C[k] = o
  }
  if (mode == 1) report(f, bline, L[bline], "unterminated block comment: the rest of the file cannot be scanned")
  else if (mode >= 2) report(f, sline, L[sline], "unterminated string: the rest of the file cannot be scanned")
}

# ---- pass 2 -------------------------------------------------------------------------
function scan(f,    k, ln, n, i, c, amode, adepth, abuf, aline, ainner) {
  amode = 0  # 0 code, 1 after `#` (and `!`), 2 inside `[...]`
  for (k = 1; k <= m; k++) {
    ln = C[k]; n = length(ln); i = 1
    # r#include matches too: `#` is not an identifier character.
    if (ln ~ /(^|[^A-Za-z0-9_])include([^A-Za-z0-9_]|$)/)
      report(f, k, L[k], "the token include: include! (also renamed by use, or r#include) makes rustc compile a file the guard does not scan; keep the code in a module under src. An identifier named include is refused too: rename it")
    while (i <= n) {
      c = substr(ln, i, 1)
      if (amode == 2) {
        if (c == "[") adepth++
        else if (c == "]") {
          adepth--
          if (adepth == 0) { attr(f, abuf, aline, ainner); amode = 0; i++; continue }
        }
        abuf = abuf c; i++; continue
      }
      if (c ~ /[ \t\f\v]/) { i++; continue }
      if (amode == 1) {
        if (c == "!" && !ainner) { ainner = 1; i++; continue }
        if (c == "[") { amode = 2; adepth = 1; abuf = ""; i++; continue }
        amode = 0   # the `#` was not an attribute
      }
      if (c == "#") { amode = 1; ainner = 0; aline = k; i++; continue }
      i++
    }
    if (amode == 2) abuf = abuf " "
  }
  if (amode == 2) report(f, aline, L[aline], "unterminated attribute: the rest of the file cannot be scanned")
}

# t: attribute text between the brackets.
function attr(f, t, ln, inner) {
  gsub(/[ \t\f\v]+/, " ", t)
  gsub(/^ | $/, "", t)
  check_attr(f, t, ln, inner, "")
}

# outer: the enclosing attribute as shown, for an attribute argument of cfg_attr.
function check_attr(f, t, ln, inner, outer,    shown, args, np, q, v, parts) {
  shown = outer != "" ? outer : (inner ? "#![" : "#[") t "]"
  gsub(/\001[0-9]+\002/, "\"...\"", shown)
  if (t ~ /^(:: ?)?([A-Za-z_][A-Za-z0-9_]* ?:: ?)*test[A-Za-z0-9_]* ?([(=]|$)/)
    report(f, ln, shown, "test attribute: a unit test in the lib or bin test exe")
  if (t ~ /^cfg ?\(/) {
    args = t; sub(/^cfg ?\(/, "", args)
    if (args ~ /(^|[(, ])(r#)?test ?([),]|$)/)
      report(f, ln, shown, "cfg predicate naming test: code for a unit-test exe")
  } else if (t ~ /^cfg_attr ?\(/) {
    args = t; sub(/^cfg_attr ?\(/, "", args); sub(/ ?\) ?$/, "", args)
    np = split_top(args, parts)
    if (parts[1] ~ /(^|[(, ])(r#)?test ?([),]|$)/)
      report(f, ln, shown, "cfg_attr predicate naming test: code for a unit-test exe")
    for (q = 2; q <= np; q++) { v = parts[q]; gsub(/^ | $/, "", v); check_attr(f, v, ln, inner, shown) }
  }
  if (t ~ /^path ?=/)
    report(f, ln, shown, "#[path] module: rustc compiles a file the guard does not scan; keep modules under src without #[path]")
}

# Split s at top-level commas into parts[1..n]; returns n.
function split_top(s, parts,    n, i, c, d, cur) {
  n = 0; d = 0; cur = ""
  for (i = 1; i <= length(s); i++) {
    c = substr(s, i, 1)
    if (c == "(" || c == "[" || c == "{") d++
    else if (c == ")" || c == "]" || c == "}") d--
    if (c == "," && d == 0) { parts[++n] = cur; cur = ""; continue }
    cur = cur c
  }
  parts[++n] = cur
  return n
}
