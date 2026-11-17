#!/usr/bin/env bash
# T-216 (decisions #292, #295, #296; docs/decisions/ci-toolchain.md): the contour dispatcher's busy count, the
# only place that decides whether teamwright-next.yml may start a stage of <TASK_ID>.
#
# Usage: gh run list --workflow teamwright-agent.yml --json status,displayTitle |
#          scripts/ci/contour-busy.sh <TASK_ID> <LIMIT>
#   stdin: a JSON array of run objects. A run is unfinished when its status is not "completed". Its
#   task is the id in the title ^teamwright agent (T-[0-9]{3,}) (the run-name of teamwright-agent.yml),
#   compared whole (T-2160 is not T-216); a title that does not parse counts as one more other task
#   (fail-closed: an unknown run may hold a runner).
#   stdout, one line, exit 0:
#     own        an unfinished run of <TASK_ID> exists (the caller waits for it to end)
#     full <n>   no own run; n other tasks have unfinished runs and n >= <LIMIT> (start nothing)
#     free <n>   no own run; n < <LIMIT> (start the stage)
# Exit 2: bad arguments (exactly <TASK_ID> ^T-[0-9]{3,}$ and <LIMIT> a positive integer). Exit 3: stdin
# is not one JSON array of objects, each with a string status. Neither prints a verdict.
# Host bash and jq (the dispatcher's runner, the gate host).
set -uo pipefail

name="contour-busy"
[ "$#" -eq 2 ] || { echo "usage: $name <TASK_ID> <LIMIT>  (gh run list JSON on stdin)" >&2; exit 2; }
task="$1" limit="$2"
[[ "$task" =~ ^T-[0-9]{3,}$ ]] || { echo "$name: bad task id (want T-NNN)" >&2; exit 2; }
[[ "$limit" =~ ^[1-9][0-9]{0,5}$ ]] || { echo "$name: bad limit (want a positive integer)" >&2; exit 2; }

verdict="$(jq -r -s --arg t "$task" --argjson lim "$limit" '
  if length != 1 then error("want one JSON value") else .[0] end
  | if type != "array" then error("not an array") else . end
  | map(if type != "object" or (.status | type) != "string" then error("not a run object") else . end)
  | map(select(.status != "completed")
        | if (.displayTitle | type) == "string"
          then ((.displayTitle | capture("^teamwright agent (?<id>T-[0-9]{3,}) ") | .id) // null)
          else null end)
  | if any(.[]; . == $t) then "own"
    else ((map(select(. != null)) | unique | length) + (map(select(. == null)) | length)) as $n
      | if $n >= $lim then "full \($n)" else "free \($n)" end
    end
')" || { echo "$name: stdin is not gh run list JSON (an array of runs with a status)" >&2; exit 3; }
[[ "$verdict" =~ ^(own|full\ [0-9]+|free\ [0-9]+)$ ]] || { echo "$name: no verdict from the run list" >&2; exit 3; }
printf '%s\n' "$verdict"
