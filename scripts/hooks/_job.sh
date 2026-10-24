# shellcheck shell=bash
# _job.sh — run a long command detached from the caller and wait for it within a budget.
# Sourced by scripts/tw-gate and scripts/tw-mutants. Bash 3.2 compatible.
#
# WHY. Claude Code's Bash tool stops one command after 10 minutes. A full gate or a
# mutant catalog can take longer: the call was cut, the agent killed the test container
# and started again, or polled the log in `until grep ...; sleep` loops. The run itself
# now lives in its own session and process group (python3's start_new_session), writes
# its output to files, and survives the caller; the caller waits up to a budget
# (TW_WAIT_SECONDS, default 540 s) and otherwise says RUNNING and exits 75, and calling
# the same command again attaches to the same run instead of starting another.
#
# A job is a directory (created with mkdir, which is atomic: it is also the lock):
#   pid       the detached process (leader of its own process group)
#   key       what it runs on; the caller attaches only to a job with its own key
#   started   UTC start time;  log  where its output goes;  what  one line for humans
#   out       the detached process's own stdout/stderr
#   rc        written last, when the run finished: the job is then "done", and the next
#             caller with the same key prints its result and removes the directory
# States: none | running | done | stale (the process is gone and left no rc: killed).

export TW_JOB_EXIT_RUNNING=75   # EX_TEMPFAIL: still running, call the same command again

job_get() { cat "$1/$2" 2>/dev/null; }

job_age() {  # dir -> seconds since it was created
  python3 -c 'import os,sys,time; print(int(time.time()-os.path.getmtime(sys.argv[1])))' "$1" 2>/dev/null || echo 0
}

job_pid_alive() {  # pid -> 0 when the process exists and is not a zombie
  local st
  kill -0 "$1" 2>/dev/null || return 1
  st="$(ps -o stat= -p "$1" 2>/dev/null | tr -d ' ')" || st=""
  case "$st" in Z*) return 1 ;; esac
  return 0
}

job_state() {  # dir -> none | running | done | stale
  local d="$1" pid
  [ -d "$d" ] || { echo none; return; }
  [ -f "$d/rc" ] && { echo "done"; return; }
  pid="$(job_get "$d" pid)"
  if [ -z "$pid" ]; then   # being created by another caller right now, or abandoned
    if [ "$(job_age "$d")" -lt 30 ]; then echo running; else echo stale; fi
    return
  fi
  if job_pid_alive "$pid"; then echo running
  elif [ -f "$d/rc" ]; then echo "done"
  else echo stale; fi
}

job_spawn() {  # dir cmd [args...] -> runs cmd detached (own session), pid in dir/pid
  python3 -c '
import os, subprocess, sys
d = sys.argv[1]
out = open(os.path.join(d, "out"), "ab")
p = subprocess.Popen(sys.argv[2:], stdin=subprocess.DEVNULL, stdout=out, stderr=subprocess.STDOUT,
                     start_new_session=True, close_fds=True)
with open(os.path.join(d, "pid.tmp"), "w") as f:
    f.write(str(p.pid))
os.replace(os.path.join(d, "pid.tmp"), os.path.join(d, "pid"))
' "$@"
}

job_finish() {  # dir rc -> marks the job done (rc written last, atomically)
  printf '%s\n' "$2" > "$1/rc.tmp" && mv "$1/rc.tmp" "$1/rc"
}

job_wait() {  # dir deadline(SECONDS) -> 0 done, 1 stale, 2 still running, 3 gone
  local d="$1" end="$2" s n=0
  while :; do
    s="$(job_state "$d")"
    case "$s" in done) return 0 ;; stale) return 1 ;; none) return 3 ;; esac
    [ "$SECONDS" -ge "$end" ] && return 2
    n=$((n + 1))
    if [ -n "${TW_POLL_SECONDS:-}" ]; then sleep "$TW_POLL_SECONDS"
    elif [ "$n" -le 10 ]; then sleep 0.2   # short runs answer fast
    else sleep 1; fi
  done
}

job_stop() {  # dir -> TERM to the job's process group
  local pid
  pid="$(job_get "$1" pid)"
  [ -n "$pid" ] || return 1
  kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null
}

job_wait_budget() {  # -> seconds to wait: TW_WAIT_SECONDS (default 540, under the 10-minute tool limit)
  local w="${TW_WAIT_SECONDS:-540}"
  case "$w" in ''|*[!0-9]*) w=540 ;; esac
  echo "$w"
}
