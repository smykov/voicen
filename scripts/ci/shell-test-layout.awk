# T-035 source scan for scripts/ci/shell-test-layout.sh (docs/decisions/ci-toolchain.md).
# Input: the shell's *.rs files. Output: one line per finding, "FILE:LINE: TEXT  (NOTE)";
# no output means no finding. POSIX awk (gawk, mawk, busybox); run with LC_ALL=C.
#
# Pass 1 (lex) reads a file the way rustc tokenizes it, as far as the checks need: plain
# comments are dropped, nested block comments included; string, raw-string and char
# literals become placeholders, so text inside them is never read as an attribute; doc
# comments (/// and //! lines, /** */ and /*! */ blocks) become doc placeholders.
# Pass 2 (scan) finds every attribute `#[...]` / `#![...]` anywhere on a line, with any
# whitespace or line breaks inside it, and groups doc fragments (doc comments and
# `doc = "..."` attributes) into doc blocks the way rustdoc joins them.
#   attribute: test attribute (name `test*`, any path) or a cfg/cfg_attr predicate naming
#              `test`; cfg_attr's attribute arguments are checked the same way;
#              `doc = <not a string literal>` (include_str!, concat!, a macro variable);
#   doc block: a fenced code block (``` or ~~~, also after a list or quote marker) other
#              than ```text; an indented (4+ columns) code block, i.e. an indented line
#              after a blank doc line, a heading, a fence or the block start (an indented
#              line right after paragraph text is a lazy continuation, not code).
# Text the lexer cannot close (block comment, string, attribute) is a finding too.

BEGIN { SO = "\001"; SE = "\002"; DO = "\003"; nstr = 0 }

FNR == 1 && NR > 1 { scan_file(cur) }
FNR == 1 { cur = FILENAME; m = 0 }
{ sub(/\r$/, ""); L[++m] = $0 }
END { if (NR > 0) scan_file(cur) }

function report(f, ln, text, note) {
  gsub(/^[ \t]+|[ \t]+$/, "", text)
  if (length(text) > 120) text = substr(text, 1, 117) "..."
  printf "%s:%d: %s  (%s)\n", f, ln, text, note
}

function newstr(kind, ln, isblock) {
  nstr++
  S[nstr] = ""; SK[nstr] = kind; SL[nstr] = ln; SB[nstr] = isblock
  return nstr
}

function scan_file(f) {
  nstr = 0
  lex(f)
  scan(f)
}

# ---- pass 1 -------------------------------------------------------------------------
# C[k]: code of source line k; plain comments removed, literals and doc comments replaced
# by SO idx SE (string) or DO idx SE (doc comment), content in S[idx].
function lex(f,    k, ln, n, i, c, nx, o, j, h, p, pp, rest, mode, bdepth, bidx, bline, sidx, sline, rterm, t) {
  mode = 0  # 0 code, 1 block comment, 2 string, 3 raw string
  for (k = 1; k <= m; k++) {
    ln = L[k]; n = length(ln); i = 1; o = ""
    while (i <= n) {
      if (mode == 1) {
        t = substr(ln, i, 2)
        if (t == "/*") { bdepth++; if (bidx) S[bidx] = S[bidx] t; i += 2; continue }
        if (t == "*/") {
          bdepth--
          if (bdepth == 0) { mode = 0; i += 2; continue }
          if (bidx) S[bidx] = S[bidx] t
          i += 2; continue
        }
        if (bidx) S[bidx] = S[bidx] substr(ln, i, 1)
        i++; continue
      }
      if (mode == 2) {
        c = substr(ln, i, 1)
        if (c == "\\") { S[sidx] = S[sidx] substr(ln, i, 2); i += 2; continue }
        if (c == "\"") { mode = 0; i++; continue }
        S[sidx] = S[sidx] c; i++; continue
      }
      if (mode == 3) {
        if (substr(ln, i, length(rterm)) == rterm) { mode = 0; i += length(rterm); continue }
        S[sidx] = S[sidx] substr(ln, i, 1); i++; continue
      }
      c = substr(ln, i, 1)
      if (c == "/") {
        nx = substr(ln, i + 1, 1)
        if (nx == "/") {
          rest = substr(ln, i)
          if (rest ~ /^\/\/\/([^\/]|$)/) {
            j = newstr("outer", k, 0); S[j] = substr(rest, 4); o = o DO j SE
          } else if (rest ~ /^\/\/!/) {
            j = newstr("inner", k, 0); S[j] = substr(rest, 4); o = o DO j SE
          } else o = o " "
          i = n + 1; continue
        }
        if (nx == "*") {
          rest = substr(ln, i)
          mode = 1; bdepth = 1; bidx = 0; bline = k
          if (rest ~ /^\/\*\*/ && rest !~ /^\/\*\*\*/ && rest !~ /^\/\*\*\//) {
            bidx = newstr("outer", k, 1); o = o DO bidx SE; i += 3
          } else if (rest ~ /^\/\*!/) {
            bidx = newstr("inner", k, 1); o = o DO bidx SE; i += 3
          } else { o = o " "; i += 2 }
          continue
        }
      }
      if (c == "\"") {
        sidx = newstr("str", k, 0); sline = k; o = o SO sidx SE; mode = 2; i++; continue
      }
      if (c == "r") {
        p = substr(ln, i - 1, 1); pp = substr(ln, i - 2, 1)
        if (p !~ /[A-Za-z0-9_]/ || (p ~ /[bc]/ && pp !~ /[A-Za-z0-9_]/)) {
          j = i + 1; h = ""
          while (substr(ln, j, 1) == "#") { h = h "#"; j++ }
          if (substr(ln, j, 1) == "\"") {
            sidx = newstr("raw", k, 0); sline = k; o = o SO sidx SE
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
    if (mode == 1 && bidx) S[bidx] = S[bidx] "\n"
    else if (mode >= 2) S[sidx] = S[sidx] "\n"
    C[k] = o
  }
  if (mode == 1) report(f, bline, L[bline], "unterminated block comment: the rest of the file cannot be scanned")
  else if (mode >= 2) report(f, sline, L[sline], "unterminated string: the rest of the file cannot be scanned")
}

# ---- pass 2 -------------------------------------------------------------------------
function scan(f,    k, ln, n, i, c, j, idx, amode, adepth, abuf, aline, ainner) {
  nb = 0; bkind = ""
  amode = 0  # 0 code, 1 after `#` (and `!`), 2 inside `[...]`
  for (k = 1; k <= m; k++) {
    ln = C[k]; n = length(ln); i = 1
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
        amode = 0; flush(f)   # the `#` was not an attribute
      }
      if (c == DO) {
        j = index(substr(ln, i), SE); idx = substr(ln, i + 1, j - 2) + 0
        frag(f, S[idx], SK[idx], SL[idx], SB[idx], 0)
        i += j; continue
      }
      if (c == "#") { amode = 1; ainner = 0; aline = k; i++; continue }
      flush(f); i++
    }
    if (amode == 2) abuf = abuf " "
  }
  if (amode == 2) report(f, aline, L[aline], "unterminated attribute: the rest of the file cannot be scanned")
  flush(f)
}

# t: attribute text between the brackets.
function attr(f, t, ln, inner) {
  gsub(/[ \t\f\v]+/, " ", t)
  gsub(/^ | $/, "", t)
  check_attr(f, t, ln, inner, "")
}

# outer: the enclosing attribute as shown, for an attribute argument of cfg_attr.
function check_attr(f, t, ln, inner, outer,    shown, args, np, q, v, idx, kind, parts) {
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
  if (t ~ /^doc ?=/) {
    v = t; sub(/^doc ?= ?/, "", v)
    kind = inner ? "inner" : "outer"
    if (v ~ /^\001[0-9]+\002$/) {
      idx = substr(v, 2, length(v) - 2) + 0
      frag(f, (SK[idx] == "raw" ? S[idx] : unescape(S[idx])), kind, SL[idx], 0, 1)
    } else
      report(f, ln, shown, "doc text not from a string literal (include_str!, a macro) can hold doctests")
  }
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

function unescape(s,    o, i, c, n, x) {
  o = ""; n = length(s)
  for (i = 1; i <= n; i++) {
    c = substr(s, i, 1)
    if (c != "\\") { o = o c; continue }
    x = substr(s, i + 1, 1)
    if (x == "n") o = o "\n"
    else if (x == "t") o = o "\t"
    else if (x == "r" || x == "0") o = o ""
    else if (x == "x") { o = o "?"; i += 2 }
    else if (x == "u") { o = o "?"; while (i <= n && substr(s, i + 1, 1) != "}") i++ }
    else if (x == "\n") { i++; while (substr(s, i + 1, 1) ~ /[ \t\n]/) i++; continue }
    else o = o x
    i++
  }
  return o
}

# Append a doc fragment to the current doc block (a change between inner and outer docs
# starts a new block).
function frag(f, text, kind, ln, isblock, isattr,    q, r, a, b, z, star) {
  if (nb > 0 && bkind != kind) flush(f)
  bkind = kind
  q = split(text, FR, "\n")
  if (q == 0) { q = 1; FR[1] = "" }
  a = 1; b = q
  if (isblock) {
    # rustc's beautify_doc_string: drop a blank first and last line; strip a ` * ` margin.
    if (FR[a] ~ /^[ \t]*$/ && a < b) a++
    if (FR[b] ~ /^[ \t]*\**[ \t]*$/ && b > a) b--
    star = 1
    for (r = a + 1; r <= b; r++) if (FR[r] !~ /^[ \t]*\*/) star = 0
    if (star) for (r = a; r <= b; r++) sub(/^[ \t]*\*/, "", FR[r])
  }
  for (r = a; r <= b; r++) {
    z = FR[r]
    while (match(z, /^ *\t/)) z = substr(z, 1, RLENGTH - 1) "    " substr(z, RLENGTH + 1)
    BL[++nb] = z
    BN[nb] = isattr ? ln : ln + r - 1
  }
}

function strip_container(u) {
  sub(/^[ \t]*(>[ \t]*|([-*+]|[0-9]+[.)])[ \t]+)*/, "", u)
  return u
}

# Analyze the doc block BL[1..nb] (rustdoc unindents it first), then empty it.
function flush(f,    r, t, u, minind, ind, prev, infence, inind, fc, fl, info) {
  if (nb == 0) return
  minind = -1
  for (r = 1; r <= nb; r++)
    if (BL[r] !~ /^[ \t]*$/) { match(BL[r], /^ */); if (minind < 0 || RLENGTH < minind) minind = RLENGTH }
  if (minind > 0) for (r = 1; r <= nb; r++) BL[r] = substr(BL[r], minind + 1)
  prev = "start"; infence = 0; inind = 0
  for (r = 1; r <= nb; r++) {
    t = BL[r]
    if (infence) {
      u = strip_container(t)
      if (match(u, /^(```+|~~~+)/) && substr(u, 1, 1) == fc && RLENGTH >= fl && substr(u, RLENGTH + 1) ~ /^[ \t]*$/) {
        infence = 0; prev = "fence"
      }
      continue
    }
    if (t ~ /^[ \t]*$/) { prev = "blank"; continue }
    match(t, /^ */); ind = RLENGTH
    if (ind >= 4) {
      if (prev == "start" || prev == "blank" || prev == "heading" || prev == "fence" || prev == "code") {
        if (!inind) report(f, BN[r], t, "indented doc code block: a doctest")
        inind = 1; prev = "code"; continue
      }
      inind = 0; prev = "text"; continue  # lazy continuation of a paragraph
    }
    inind = 0
    u = strip_container(t)
    if (match(u, /^(```+|~~~+)/)) {
      fc = substr(u, 1, 1); fl = RLENGTH; info = substr(u, RLENGTH + 1)
      gsub(/^[ \t]+|[ \t]+$/, "", info)
      if (!(fc == "`" && index(info, "`") > 0)) {
        if (info != "text") report(f, BN[r], t, "doc code fence: a doctest; only ```text is allowed")
        infence = 1; prev = "fence"; continue
      }
    }
    prev = (t ~ /^#+( |$)/) ? "heading" : "text"
  }
  nb = 0; bkind = ""
}
