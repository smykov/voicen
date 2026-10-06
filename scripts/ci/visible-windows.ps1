# T-037: the windows a user can see for one process, for the install smoke in
# .github/workflows/ci.yml (dot-sourced by the first-launch and the loaded-launch steps, so both
# decide on the same predicate). Windows only; pwsh.
#
# A shown window is a top-level window of the process that is visible (IsWindowVisible), has no
# owner (GetWindow GW_OWNER) and is not a framework helper window. Tool windows (WS_EX_TOOLWINDOW)
# count (T-057, F-003: the predicate decides on raw window facts, so an overlay or any other tool
# window shown at start fails the smoke).
#
# A helper window is excluded only when two raw facts both hold (T-057 VERIFY_FAIL 3; T-065):
#  (a) its class is a visible-helper class of the helper-window manifest helper-windows.txt next
#      to this script (one entry per crate@version of the shell's Windows dependency graph that
#      mentions CreateWindowEx, each with its verdict, class pattern and source citation; a
#      `{identifier}` in a class is src-tauri/tauri.conf.json `identifier`). The manifest is the one
#      source of the helper list: this script, the census below and the shell tests
#      (src-tauri/tests/helper_windows/mod.rs) read it, and make check
#      (scripts/ci/helper-windows.sh) fails when it differs from the dependency graph;
#  (b) its extended style has all four helper bits, WS_EX_LAYERED | WS_EX_TRANSPARENT |
#      WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW (0x080800A0).
# The listed visible helpers are created top-level and unowned with exactly those four bits and
# 0x0 size, then get GWL_STYLE = WS_VISIBLE | WS_POPUP, so IsWindowVisible is true for them and
# Process.MainWindowHandle may return one (T-037 review 1 #1; T-057 VERIFY_FAIL 3). Nothing is
# excluded by style alone (tao maps click-through to WS_EX_TRANSPARENT | WS_EX_LAYERED, so a
# click-through overlay would drop out), by size or by title, and there is no exception for our
# own windows: a product window (class "Tauri Window") never matches (a), a listed class that
# changes shape counts again, and a hidden-helper class that turns up visible counts. WebView2's
# own windows are child windows or belong to msedgewebview2.exe.
#
# Get-ShownWindows -ProcessId <pid> [-Manifest <path>] returns objects with Handle, Class, Title
# (empty when none) and ExStyle (GWL_EXSTYLE). Format-ShownWindows formats them for a log line,
# ex-style in hex.
#
# T-065 census: Get-HelperDrift -ProcessId <pid> [-Manifest <path>] compares the manifest's
# visible-helper verdicts with the real window set, both ways. It returns one object per drift:
# Kind 'unlisted' (a visible, unowned, top-level window with the four helper bits whose class is
# not a visible-helper class of the manifest; it is counted as shown, never excluded for its
# shape) or 'stale' (a visible-helper class of the manifest with no such window), with Class,
# Title and ExStyle (0 for stale). Format-HelperDrift formats them as one "helper list drift" line.
# The install smoke fails on any drift before it counts the shown windows, so a dependency change
# that adds, removes or reshapes a helper is reported as what it is, not as a stray product window.
#
# T-052 (raw facts only, F-003): Get-AllWindows -ProcessId <pid>
# returns every top-level window of the process (Handle, Class, Title, Visible), so a step can
# find tray-icon's hidden 'tray_icon_app' window; Test-Iconic, Invoke-Minimize and Send-Close
# (WM_CLOSE, posted) act on one HWND.

# The default manifest and the config its {identifier} comes from, next to this script.
$VoicenHelperManifest = Join-Path $PSScriptRoot 'helper-windows.txt'
$VoicenTauriConfig = [System.IO.Path]::Combine($PSScriptRoot, '..', '..', 'src-tauri', 'tauri.conf.json')

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public sealed class VoicenShownWindow {
  public IntPtr Handle;
  public string Class;
  public string Title;
  public uint ExStyle;
}

public sealed class VoicenWindow {
  public IntPtr Handle;
  public string Class;
  public string Title;
  public bool Visible;
}

public static class VoicenWindows {
  delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll")] static extern IntPtr GetWindow(IntPtr hWnd, uint cmd);
  [DllImport("user32.dll")] static extern int GetWindowLongW(IntPtr hWnd, int index);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr hWnd, StringBuilder name, int max);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr hWnd, StringBuilder text, int max);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int cmd);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
  public const int SW_MINIMIZE = 6;
  public const uint WM_CLOSE = 0x0010;
  const uint GW_OWNER = 4;
  const int GWL_EXSTYLE = -20;
  // WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW: the shape every
  // visible helper of the manifest is created with (citations in helper-windows.txt).
  const uint HelperExStyle = 0x00080000u | 0x00000020u | 0x08000000u | 0x00000080u;

  static bool HasHelperBits(uint exStyle) {
    return (exStyle & HelperExStyle) == HelperExStyle;
  }

  // Visible, unowned, top-level windows of the process (class, title and ex-style read).
  static List<VoicenShownWindow> Candidates(uint pid) {
    var found = new List<VoicenShownWindow>();
    EnumWindowsProc cb = (hWnd, lParam) => {
      uint owner;
      GetWindowThreadProcessId(hWnd, out owner);
      if (owner != pid || !IsWindowVisible(hWnd) || GetWindow(hWnd, GW_OWNER) != IntPtr.Zero) return true;
      var cls = new StringBuilder(256);
      GetClassNameW(hWnd, cls, cls.Capacity);
      uint ex = unchecked((uint)GetWindowLongW(hWnd, GWL_EXSTYLE));
      var title = new StringBuilder(512);
      GetWindowTextW(hWnd, title, title.Capacity);
      found.Add(new VoicenShownWindow { Handle = hWnd, Class = cls.ToString(), Title = title.ToString(), ExStyle = ex });
      return true;
    };
    EnumWindows(cb, IntPtr.Zero);
    GC.KeepAlive(cb);
    return found;
  }

  // The shown windows: every candidate but those whose class is one of helperClasses (the
  // manifest's visible-helper classes, compared ordinally) AND that have all four helper bits.
  public static VoicenShownWindow[] Shown(uint pid, string[] helperClasses) {
    var classes = helperClasses ?? new string[0];
    return Candidates(pid).FindAll(w => !(Array.IndexOf(classes, w.Class) >= 0 && HasHelperBits(w.ExStyle))).ToArray();
  }

  // The helper-shaped windows: every candidate with all four helper bits, whatever its class.
  public static VoicenShownWindow[] HelperShaped(uint pid) {
    return Candidates(pid).FindAll(w => HasHelperBits(w.ExStyle)).ToArray();
  }

  public static VoicenWindow[] All(uint pid) {
    var all = new List<VoicenWindow>();
    EnumWindowsProc cb = (hWnd, lParam) => {
      uint owner;
      GetWindowThreadProcessId(hWnd, out owner);
      if (owner != pid) return true;
      var cls = new StringBuilder(256);
      GetClassNameW(hWnd, cls, cls.Capacity);
      var title = new StringBuilder(512);
      GetWindowTextW(hWnd, title, title.Capacity);
      all.Add(new VoicenWindow { Handle = hWnd, Class = cls.ToString(), Title = title.ToString(), Visible = IsWindowVisible(hWnd) });
      return true;
    };
    EnumWindows(cb, IntPtr.Zero);
    GC.KeepAlive(cb);
    return all.ToArray();
  }
}
'@

# The visible-helper classes of the manifest at $Path ({identifier} resolved). Throws on a missing
# or malformed manifest: an unreadable list never reads as "no helpers".
function Get-VisibleHelperClasses([string]$Path) {
  if ([string]::IsNullOrEmpty($Path) -or -not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    throw "helper-window manifest '$Path' not found"
  }
  $identifier = $null
  $classes = [System.Collections.Generic.List[string]]::new()
  $n = 0
  foreach ($raw in [System.IO.File]::ReadAllLines($Path)) {
    $n++
    $line = $raw.Trim()
    if ($line -eq '' -or $line.StartsWith('#')) { continue }
    $fields = $line.Split([char]'|')
    if ($fields.Count -ne 4) { throw "helper-window manifest '$Path' line ${n}: want <crate>@<version> | <verdict> | <class or -> | <citation>" }
    $verdict = $fields[1].Trim()
    $class = $fields[2].Trim()
    if ($verdict -cne 'visible-helper' -and $verdict -cne 'hidden-helper' -and $verdict -cne 'child' -and $verdict -cne 'binding') {
      throw "helper-window manifest '$Path' line ${n}: verdict '$verdict' is not visible-helper, hidden-helper, child or binding"
    }
    if ($verdict -cne 'visible-helper') { continue }
    if ($class -eq '' -or $class -eq '-') { throw "helper-window manifest '$Path' line ${n}: a visible-helper entry names its class" }
    if ($class.Contains('{identifier}')) {
      if ($null -eq $identifier) {
        $identifier = (Get-Content -LiteralPath $VoicenTauriConfig -Raw | ConvertFrom-Json).identifier
        if ([string]::IsNullOrEmpty($identifier)) { throw "no identifier in $VoicenTauriConfig (helper-window manifest '$Path' line ${n})" }
      }
      $class = $class.Replace('{identifier}', $identifier)
    }
    $classes.Add($class)
  }
  , $classes.ToArray()
}

function Get-ShownWindows {
  param([Parameter(Mandatory = $true)][int]$ProcessId, [string]$Manifest = $VoicenHelperManifest)
  [string[]]$classes = Get-VisibleHelperClasses $Manifest
  , [VoicenWindows]::Shown([uint32]$ProcessId, $classes)
}

# The census (T-065): one object per drift between the manifest's visible-helper verdicts and the
# real window set of the process, written to the pipeline (none when they agree).
function Get-HelperDrift {
  param([Parameter(Mandatory = $true)][int]$ProcessId, [string]$Manifest = $VoicenHelperManifest)
  [string[]]$classes = Get-VisibleHelperClasses $Manifest
  $shaped = [VoicenWindows]::HelperShaped([uint32]$ProcessId)
  $present = [System.Collections.Generic.List[string]]::new()
  foreach ($w in $shaped) {
    $present.Add($w.Class)
    if ([Array]::IndexOf($classes, $w.Class) -lt 0) {
      [pscustomobject]@{ Kind = 'unlisted'; Class = $w.Class; Title = $w.Title; ExStyle = $w.ExStyle }
    }
  }
  foreach ($c in $classes) {
    if (-not $present.Contains($c)) {
      [pscustomobject]@{ Kind = 'stale'; Class = $c; Title = ''; ExStyle = [uint32]0 }
    }
  }
}

function Format-HelperDrift($Drift) {
  $items = @($Drift)
  if ($items.Count -eq 0) { return 'none' }
  $parts = $items | ForEach-Object {
    if ($_.Kind -ceq 'stale') { "stale: class '$($_.Class)' is a visible helper in the manifest, but the process has no visible, unowned, top-level window of that class with the four helper bits" }
    else { "unlisted: class '$($_.Class)', title '$($_.Title)', ex-style 0x$(([uint32]$_.ExStyle).ToString('X8')) is visible, unowned, top-level with the four helper bits, and not a visible helper in the manifest" }
  }
  "helper list drift (scripts/ci/helper-windows.txt against the real window set; re-read the crate's source, fix the entry's verdict, docs/decisions/overlay.md §5): " + ($parts -join '; ')
}

function Format-ShownWindows($Windows) {
  if (-not $Windows -or $Windows.Count -eq 0) { return 'none' }
  ($Windows | ForEach-Object { "class '$($_.Class)', title '$($_.Title)', ex-style 0x$($_.ExStyle.ToString('X8'))" }) -join '; '
}

function Get-AllWindows([int]$ProcessId) { , [VoicenWindows]::All([uint32]$ProcessId) }

function Format-AllWindows($Windows) {
  if (-not $Windows -or $Windows.Count -eq 0) { return 'none' }
  ($Windows | ForEach-Object { "class '$($_.Class)', title '$($_.Title)', visible $($_.Visible)" }) -join '; '
}

function Test-Iconic([IntPtr]$Handle) { [VoicenWindows]::IsIconic($Handle) }

function Invoke-Minimize([IntPtr]$Handle) { [void][VoicenWindows]::ShowWindow($Handle, [VoicenWindows]::SW_MINIMIZE) }

function Send-Close([IntPtr]$Handle) {
  if (-not [VoicenWindows]::PostMessageW($Handle, [VoicenWindows]::WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero)) { throw "PostMessageW(WM_CLOSE) failed for window $Handle" }
}
