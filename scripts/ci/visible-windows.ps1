# T-037: the windows a user can see for one process, for the install smoke in
# .github/workflows/ci.yml (dot-sourced by the first-launch and the loaded-launch steps, so both
# decide on the same predicate). Windows only; pwsh.
#
# A shown window is a top-level window of the process that is visible (IsWindowVisible), has no
# owner (GetWindow GW_OWNER) and is not a framework helper window. Tool windows (WS_EX_TOOLWINDOW)
# count (T-057, F-003: the predicate decides on raw window facts, so an overlay or any other tool
# window shown at start fails the smoke).
#
# A helper window is excluded only when two raw facts both hold (T-057 VERIFY_FAIL 3):
#  (a) its class is on the pinned helper list below, one entry per pinned framework source that
#      creates an always-visible helper window:
#      - "Tao Thread Event Target": tao 0.37.1, src/platform_impl/windows/event_loop.rs:629-687
#        (create_event_target_window), in every tauri process;
#      - "dev.voicen.app-sic": tauri-plugin-single-instance 2.5.2, src/platform_impl/windows.rs:66-67
#        ("{identifier}-sic", identifier from src-tauri/tauri.conf.json; built without the
#        'semver' feature, so no version suffix) and :203-232 (create_event_target_window);
#  (b) its extended style has all four helper bits, WS_EX_LAYERED | WS_EX_TRANSPARENT |
#      WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW (0x080800A0).
# Both sources create the window top-level and unowned with exactly those four bits and 0x0 size,
# then set GWL_STYLE = WS_VISIBLE | WS_POPUP, so IsWindowVisible is true for it and
# Process.MainWindowHandle may return it (T-037 review 1 #1: tao's; T-057 VERIFY_FAIL 3: the
# plugin's). Nothing is excluded by style alone (tao maps click-through to WS_EX_TRANSPARENT |
# WS_EX_LAYERED, so a click-through overlay would drop out), by size or by title, and there is no
# exception for our own windows: a product window (class "Tauri Window") never matches (a), and a
# listed class that changes shape counts again. A helper the list does not know fails the smoke
# loudly; Format-ShownWindows names its class and ex-style. If the identifier in tauri.conf.json
# changes, the plugin entry changes with it (smoke_predicate.rs reads the identifier from the
# config and fails on a drift). WebView2's own windows are child windows or belong to
# msedgewebview2.exe.
#
# Get-ShownWindows -ProcessId <pid> returns objects with Handle, Class, Title (empty when none)
# and ExStyle (GWL_EXSTYLE). Format-ShownWindows formats them for a log line, ex-style in hex.
#
# T-052 (raw facts only, F-003): Get-AllWindows -ProcessId <pid>
# returns every top-level window of the process (Handle, Class, Title, Visible), so a step can
# find tray-icon's hidden 'tray_icon_app' window; Test-Iconic, Invoke-Minimize and Send-Close
# (WM_CLOSE, posted) act on one HWND.

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
  // WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW: the shape both pinned
  // helper sources create (see the header for the citations).
  const uint HelperExStyle = 0x00080000u | 0x00000020u | 0x08000000u | 0x00000080u;
  // The pinned helper classes, one per framework source (citations in the header).
  static readonly string[] HelperClasses = {
    "Tao Thread Event Target", // tao 0.37.1 event_loop.rs create_event_target_window
    "dev.voicen.app-sic",      // tauri-plugin-single-instance 2.5.2 windows.rs, "{identifier}-sic"
  };

  static bool IsHelper(string cls, uint exStyle) {
    return Array.IndexOf(HelperClasses, cls) >= 0 && (exStyle & HelperExStyle) == HelperExStyle;
  }

  public static VoicenShownWindow[] Shown(uint pid) {
    var shown = new List<VoicenShownWindow>();
    EnumWindowsProc cb = (hWnd, lParam) => {
      uint owner;
      GetWindowThreadProcessId(hWnd, out owner);
      if (owner != pid || !IsWindowVisible(hWnd) || GetWindow(hWnd, GW_OWNER) != IntPtr.Zero) return true;
      var cls = new StringBuilder(256);
      GetClassNameW(hWnd, cls, cls.Capacity);
      uint ex = unchecked((uint)GetWindowLongW(hWnd, GWL_EXSTYLE));
      if (IsHelper(cls.ToString(), ex)) return true;
      var title = new StringBuilder(512);
      GetWindowTextW(hWnd, title, title.Capacity);
      shown.Add(new VoicenShownWindow { Handle = hWnd, Class = cls.ToString(), Title = title.ToString(), ExStyle = ex });
      return true;
    };
    EnumWindows(cb, IntPtr.Zero);
    GC.KeepAlive(cb);
    return shown.ToArray();
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

function Get-ShownWindows([int]$ProcessId) { , [VoicenWindows]::Shown([uint32]$ProcessId) }

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
