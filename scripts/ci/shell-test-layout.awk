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
# `doc = "..."` attributes) into doc blocks; the blocks of all files are read at the end.
#   attribute: test attribute (name `test*`, any path) or a cfg/cfg_attr predicate naming
#              `test`; cfg_attr's attribute arguments are checked the same way;
#              `doc = <not a string literal>` (include_str!, concat!, a macro variable);
#              `path = ...` (a #[path] module: a file outside the scanned set);
#   code:      the token `include` anywhere (include!, also renamed by `use` or written
#              r#include: a file outside the scanned set; identifiers that only contain
#              it, include_str! and include_bytes! pass);
#   doc block: decision #39, a coarse fail-closed rule; CommonMark list, quote and
#              paragraph context is not modelled. Each doc line is first prepared the way
#              rustc and rustdoc prepare it (block comment margins as rustc's
#              beautify_doc_string, `doc = "..."` escapes decoded, an escaped CR read as a
#              line break), then unindented by a bound rustdoc never undercuts: the
#              smallest leading whitespace of any doc line in the shell (one less on
#              `doc = "..."` lines), since rustdoc unindents an item's outer and inner docs
#              together. Outside a ```text block it then refuses: every line indented 4+
#              columns; every container marker (`>`, `-` `*` `+`, `N.` `N)`) followed by 4+
#              columns or a tab; every fence other than ```text (or ~~~text); a ```text
#              fence after a container marker; every raw HTML block line (an HTML block can
#              swallow a fence). A ```text block ends at its closing fence (indented below
#              4 columns), at a line indented less than its opening fence, or at a line of
#              the other doc form (`doc = "..."` vs comment); a fence-like line indented 4+
#              inside it is refused.
# Text the lexer cannot close (block comment, string, attribute) is a finding too.

BEGIN { SO = "\001"; SE = "\002"; DO = "\003"; nstr = 0; ng = 0; nk = 0 }

FNR == 1 && NR > 1 { scan_file(cur) }
FNR == 1 { cur = FILENAME; m = 0 }
{ sub(/\r$/, ""); L[++m] = $0 }
END { if (NR > 0) scan_file(cur); docs() }

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
  if (t ~ /^path ?=/)
    report(f, ln, shown, "#[path] module: rustc compiles a file the guard does not scan; keep modules under src without #[path]")
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

function unescape(s,    o, i, c, n, x, j, h) {
  o = ""; n = length(s)
  for (i = 1; i <= n; i++) {
    c = substr(s, i, 1)
    if (c != "\\") { o = o c; continue }
    x = substr(s, i + 1, 1)
    if (x == "n") o = o "\n"
    else if (x == "t") o = o "\t"
    else if (x == "r") o = o "\r"
    else if (x == "0") o = o "?"
    else if (x == "x") { o = o chr(hexval(substr(s, i + 2, 2))); i += 2 }
    else if (x == "u") {
      j = index(substr(s, i + 2), "}")
      h = substr(s, i + 3, j - 2); gsub(/_/, "", h)
      o = o chr(hexval(h)); i += j
    }
    else if (x == "\n") { i++; while (substr(s, i + 1, 1) ~ /[ \t\n]/) i++; continue }
    else o = o x
    i++
  }
  return o
}

function hexval(h,    v, i, d) {
  v = 0; h = tolower(h)
  for (i = 1; i <= length(h); i++) {
    d = index("0123456789abcdef", substr(h, i, 1))
    if (d == 0) return -1
    v = v * 16 + d - 1
  }
  return v
}

# A decoded escape: ASCII as itself (a fence or an indent can be spelled \x60, \u{20});
# NUL, other controls and non-ASCII as `?` (not whitespace, so never an indent).
function chr(v) {
  if (v == 9) return "\t"
  if (v == 10) return "\n"
  if (v == 13) return "\r"
  if (v >= 32 && v < 127) return sprintf("%c", v)
  return "?"
}

# rustc's beautify_doc_string for a block doc comment with more than one line, on
# FR[1..q]: drops a first line of only `*` (or empty) and a last non-empty line of only
# `*`, then, when every line between the first one starting with `*` (else the second)
# and the last non-blank one has its `*` at the same column with only spaces or tabs
# before it, strips that prefix from every kept line and then one `*` from a line that is
# `*` or starts `* ` or `**`. Sets FA..FB to the kept lines. Pinned against rustdoc 1.99.
function beautify(q,    i, j, s, e, r, c, ch, ok, star, pre, t) {
  i = 1; j = q
  if (FR[1] ~ /^\**$/) i = 2
  if (j >= i && FR[j] != "" && FR[j] ~ /^\*+$/) j--
  s = i; e = j
  if (s <= e) { t = FR[s]; sub(/^[ \t\r\f\v]+/, "", t); if (substr(t, 1, 1) != "*") s++ }
  while (s <= e && FR[s] ~ /^[ \t\r\f\v]*$/) s++
  while (e >= s && FR[e] ~ /^[ \t\r\f\v]*$/) e--
  ok = (s <= e); star = -1
  for (r = s; ok && r <= e; r++) {
    t = FR[r]
    for (c = 1; c <= length(t); c++) {
      ch = substr(t, c, 1)
      if ((star >= 0 && c - 1 > star) || index("* \t", ch) == 0) { ok = 0; break }
      if (ch == "*") { if (star < 0) star = c - 1; else if (star != c - 1) ok = 0; break }
    }
    if (ok && (star < 0 || star >= length(t))) ok = 0
  }
  if (ok) {
    pre = substr(FR[s], 1, star)
    for (r = i; r <= j; r++) {
      if (substr(FR[r], 1, star) != pre) continue
      FR[r] = substr(FR[r], star + 1)
      if (FR[r] == "*" || substr(FR[r], 1, 2) == "* " || substr(FR[r], 1, 2) == "**") FR[r] = substr(FR[r], 2)
    }
  }
  FA = i; FB = j
}

# Append a doc fragment to the current doc block (a change between inner and outer docs
# starts a new block). Lines are kept as rustdoc's lines() gives them (tabs and an
# escaped CR inside a line kept); docs() unindents and reads them.
function frag(f, text, kind, ln, isblock, isattr,    q, r) {
  if (nb > 0 && bkind != kind) flush(f)
  bkind = kind
  q = split(text, FR, "\n")
  if (q == 0) { q = 1; FR[1] = "" }
  if (q > 1 && FR[q] == "") q--   # lines() yields no empty line after a final newline
  for (r = 1; r <= q; r++) sub(/\r$/, "", FR[r])
  FA = 1; FB = q
  if (isblock && q > 1) beautify(q)
  for (r = FA; r <= FB; r++) {
    if (nb == 0) cbs = ng + 1
    GT[++ng] = FR[r]; GR[ng] = isattr; nb++
    GN[ng] = isattr ? ln : ln + r - 1
  }
}

# Close the current doc block; docs() reads it after every file is scanned.
function flush(f) {
  if (nb == 0) return
  KB[++nk] = cbs; KE[nk] = ng; KF[nk] = f
  nb = 0; bkind = ""
}

# 1 when a container marker on doc line t (indented below 4 columns) is followed by 4+
# columns or a tab and then text. CommonMark reads 5+ columns after the marker as an
# indented code block in the item or quote; 4 and a tab are refused too (fail-closed: no
# tab stops, no optional-space accounting). Markers: `>` (space optional after it), `-`
# `*` `+` and `N.` `N)` (space required after them).
function marker_code(t,    u, w) {
  u = t; sub(/^ */, "", u)
  while (match(u, /^(>|[-*+]|[0-9]+[.)])/)) {
    w = substr(u, 1, 1); u = substr(u, RLENGTH + 1)
    match(u, /^[ \t]*/)
    if (RLENGTH == 0 && w != ">") return 0  # `-x`, `1.5`: no list marker
    w = substr(u, 1, RLENGTH); u = substr(u, RLENGTH + 1)
    if (u == "") return 0
    if (index(w, "\t") > 0 || length(w) >= 4) return 1
  }
  return 0
}

function strip_container(u) {
  sub(/^[ \t]*(>[ \t]*|([-*+]|[0-9]+[.)])[ \t]+)*/, "", u)
  return u
}

# Read every doc block: unindent by U (see the header), then the coarse rule of #39.
function docs(    g, U, k) {
  U = -1
  for (g = 1; g <= ng; g++)
    if (GT[g] ~ /[^ \t\r\f\v]/) { match(GT[g], /^[ \t]*/); if (U < 0 || RLENGTH < U) U = RLENGTH }
  if (U < 0) U = 0
  for (k = 1; k <= nk; k++) doc_block(KF[k], KB[k], KE[k], U)
}

function doc_block(f, b, e, U,    g, z, u, np, p, y, nm, r, t, ind, fi, fk, fc, fl, info, infence) {
  nm = 0
  for (g = b; g <= e; g++) {
    z = GT[g]
    if (z ~ /[^ \t\r\f\v]/) { u = GR[g] ? (U > 0 ? U - 1 : 0) : U; z = substr(z, u + 1) }
    np = split(z, PC, "\r")   # Markdown reads a CR as a line break
    for (p = 1; p <= np; p++) {
      y = PC[p]
      while (match(y, /^ *\t/)) y = substr(y, 1, RLENGTH - 1) "    " substr(y, RLENGTH + 1)
      ML[++nm] = y; MN[nm] = GN[g]; MR[nm] = GR[g]
    }
  }
  infence = 0
  for (r = 1; r <= nm; r++) {
    t = ML[r]
    if (t ~ /^[ \t]*$/) continue
    match(t, /^ */); ind = RLENGTH
    if (infence) {
      if (ind >= fi && MR[r] == fk) {
        u = substr(t, ind + 1)
        if (match(u, /^(```+|~~~+)/) && substr(u, 1, 1) == fc && RLENGTH >= fl && substr(u, RLENGTH + 1) ~ /^[ \t]*$/) {
          if (ind >= 4) report(f, MN[r], t, "fence line indented 4+ columns inside a ```text block: rustdoc may close the block here and read what follows as code; indent it like the opening fence")
          infence = 0
        }
        continue
      }
      infence = 0  # rustdoc may have closed the block with its list item: read the line
    }
    if (ind >= 4) {
      report(f, MN[r], t, "doc line indented 4+ columns: rustdoc may read it as an indented code block, a doctest (it continues a list item or a paragraph only in contexts the guard does not model): indent continuations by fewer than 4 columns or write the example as ```text")
      continue
    }
    if (marker_code(t)) {
      report(f, MN[r], t, "list or quote marker followed by 4+ columns or a tab: rustdoc may read the rest as an indented code block, a doctest (also for a quoted paragraph continuation); put one space after the marker")
      continue
    }
    u = strip_container(t)
    if (u ~ /^<([A-Za-z][A-Za-z0-9-]*([ \t>\/]|$)|[\/!?])/) {
      report(f, MN[r], t, "raw HTML block in a doc comment: rustdoc's HTML block can swallow a ```text fence and leave the lines after it as code; write Markdown (a type or tag in backticks)")
      continue
    }
    if (match(u, /^(```+|~~~+)/)) {
      fc = substr(u, 1, 1); fl = RLENGTH; info = substr(u, RLENGTH + 1)
      gsub(/^[ \t]+|[ \t]+$/, "", info)
      if (fc == "`" && index(info, "`") > 0) continue  # not a fence: inline code
      if (info != "text") report(f, MN[r], t, "doc code fence: a doctest; only ```text is allowed")
      else if (length(u) != length(t) - ind) {
        report(f, MN[r], t, "```text fence after a list or quote marker: rustdoc ends it with the item or quote; put the fence on its own line, indented under the item")
        continue
      }
      infence = 1; fi = length(t) - length(u); fk = MR[r]
    }
  }
}
