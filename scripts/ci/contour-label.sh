#!/usr/bin/env bash
# T-231 (docs/decisions/ci-toolchain.md, R14): the title parser of the contour's label start. The relay job
# `start` of .github/workflows/teamwright-next.yml passes the labelled Issue's title here (through an env
# value, never expanded into a script) and dispatches hop 0 for the task id this prints.
#
# Usage: scripts/ci/contour-label.sh <TITLE>   (exactly one argument)
#   The raw bytes of the title (LC_ALL=C) are matched against the owned ERE ^(T-[0-9]{3,}): ; group 1 is
#   the only value printed and the only part of the title that reaches any command.
#     match     stdout "start <T-NNN>", exit 0
#     no match  stdout "bad <why>" (one line, no title bytes), exit 1
# Exit 2: not exactly one argument; nothing on stdout. Host bash only; the title is never evaluated.
set -u
export LC_ALL=C

[ "$#" -eq 1 ] || { echo "usage: contour-label.sh <TITLE>" >&2; exit 2; }
title="$1"
re='^(T-[0-9]{3,}):'

if [ -z "$title" ]; then
  echo "bad the title is empty"
  exit 1
fi
if [[ "$title" =~ $re ]]; then
  printf 'start %s\n' "${BASH_REMATCH[1]}"
  exit 0
fi
echo "bad the title does not start with a task id and a colon (T-NNN:)"
exit 1
