# Helpers of the T-024 crash-record smoke (ci.yml windows job): the installed app's session
# marker, crash files and log lines, and a WM_ENDSESSION sender. Dot-sourced by the crash steps.
# Every crash file is counted by name set per step: each earlier Stop-Process -Force leaves one
# (an abnormal_end file on the next launch), so a step compares the names before and after.

$CrashLogs = Join-Path $env:LOCALAPPDATA 'Voicen\logs'
$CrashExe = Join-Path $env:LOCALAPPDATA 'Voicen\voicen.exe'
$CrashMarker = Join-Path $CrashLogs 'session.marker'
# voicen.log plus rolled voicen-*.log (a run crossing local midnight rolls the file, T-008).
$CrashLogGlob = Join-Path $CrashLogs 'voicen*.log'
# The keys a crash file may hold (contracts/core-diag.md "Crash files"), and the backtrace's
# indented frame lines.
$CrashKeys = @('kind', 'time', 'version', 'commit', 'os', 'session_started', 'thread', 'location', 'backtrace', 'exception_code', 'fault_address', 'previous_session_pid')

# The crash-*.txt names in the logs dir, sorted.
function Get-CrashNames {
  , @(Get-ChildItem -LiteralPath $CrashLogs -Filter 'crash-*.txt' -File -ErrorAction SilentlyContinue | ForEach-Object { $_.Name } | Sort-Object)
}

# The names of $After that are not in $Before.
function Get-NewCrashNames($Before, $After) {
  , @($After | Where-Object { $Before -notcontains $_ })
}

# The value of "<key>: <value>" in a crash file, or $null.
function Get-CrashValue([string]$Name, [string]$Key) {
  foreach ($l in @(Get-Content -LiteralPath (Join-Path $CrashLogs $Name))) {
    if ($l.StartsWith("${Key}: ")) { return $l.Substring($Key.Length + 2) }
  }
  $null
}

# Throws unless every line of every crash file is an allowlisted "<key>: <value>", a bare
# "backtrace:" or an indented frame "  <module>+0x<hex>".
function Assert-CrashKeys($Names) {
  foreach ($n in $Names) {
    foreach ($l in @(Get-Content -LiteralPath (Join-Path $CrashLogs $n))) {
      if ($l -ceq 'backtrace:') { continue }
      if ($l -cmatch '^  [^ ]+\+0x[0-9a-f]+$') { continue }
      $key = if ($l -cmatch '^([a-z_]+): ') { $Matches[1] } else { $null }
      if (-not $key -or $CrashKeys -cnotcontains $key) { throw "crash file $n holds a line outside the allowlisted keys: '$l'" }
    }
  }
}

# The set of every line of every log file, taken before an action: the lines written since are
# the ones not in it, whatever file they are in now (a roll renames voicen.log and keeps its lines).
function Get-CrashLogMark {
  $m = [System.Collections.Generic.HashSet[string]]::new()
  foreach ($f in @(Get-ChildItem -Path $CrashLogGlob -ErrorAction SilentlyContinue)) {
    foreach ($l in @(Get-Content -LiteralPath $f.FullName)) { [void]$m.Add($l) }
  }
  , $m
}

# The log lines not in $Mark.
function Get-CrashLogLinesSince($Mark) {
  $out = [System.Collections.Generic.List[string]]::new()
  foreach ($f in @(Get-ChildItem -Path $CrashLogGlob -ErrorAction SilentlyContinue)) {
    foreach ($l in @(Get-Content -LiteralPath $f.FullName)) { if (-not $Mark.Contains($l)) { $out.Add($l) } }
  }
  , @($out)
}

# True when the marker exists and holds the line pid=<id>.
function Test-MarkerOf([int]$Id) {
  (Test-Path -LiteralPath $CrashMarker -PathType Leaf) -and (@(Get-Content -LiteralPath $CrashMarker) -ccontains "pid=$Id")
}

# Launches the installed app and waits (30 s) for its start line and for the session marker
# naming its pid: Session::begin has run, so any abnormal_end file for the previous session is
# already written. Throws (after killing it) otherwise.
function Start-VoicenSession {
  $p = Start-Process -FilePath $CrashExe -PassThru
  $null = $p.Handle
  $ok = $false
  for ($i = 0; $i -lt 60 -and -not $ok; $i++) {
    Start-Sleep -Milliseconds 500
    if ($p.HasExited) { break }
    $started = @(Get-ChildItem -Path $CrashLogGlob -ErrorAction SilentlyContinue | Select-String -Pattern "started pid=$($p.Id)(\D|$)").Count -ge 1
    $ok = $started -and (Test-MarkerOf $p.Id)
  }
  if (-not $ok) {
    $text = if (Test-Path -LiteralPath $CrashMarker) { (Get-Content -LiteralPath $CrashMarker -Raw) } else { 'missing' }
    Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
    throw "the launch (pid $($p.Id)) wrote no start line or no session marker naming it within 30 s (marker: $text)"
  }
  $p
}

if (-not ('T024.Session' -as [type])) {
  Add-Type -Namespace T024 -Name Session -MemberDefinition @'
[DllImport("user32.dll", SetLastError = true)] public static extern IntPtr SendMessageTimeoutW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam, uint flags, uint timeout, out IntPtr result);
'@
}

# Sends WM_QUERYENDSESSION, then WM_ENDSESSION(TRUE), as Windows does at log-off, to one window
# (SMTO_ABORTIFHUNG, 5 s each). The second call may return 0: the app exits inside its handler.
function Send-EndSession([IntPtr]$Handle) {
  $r = [IntPtr]::Zero
  [void][T024.Session]::SendMessageTimeoutW($Handle, 0x11, [IntPtr]::Zero, [IntPtr]::Zero, 2, 5000, [ref]$r)
  [void][T024.Session]::SendMessageTimeoutW($Handle, 0x16, [IntPtr]1, [IntPtr]::Zero, 2, 5000, [ref]$r)
}
