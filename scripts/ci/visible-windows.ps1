# T-037: the windows a user can see for one process, for the install smoke in
# .github/workflows/ci.yml (dot-sourced by the first-launch and the loaded-launch steps, so both
# decide on the same predicate). Windows only; pwsh.
#
# A shown window is a top-level window of the process that is visible (IsWindowVisible), has no
# owner (GetWindow GW_OWNER), is not a tool window (WS_EX_TOOLWINDOW) and is not of the class
# "Tao Thread Event Target". That last window exists in every tauri process: tao 0.37.1
# (src/platform_impl/windows/event_loop.rs:629-687) creates it top-level and unowned, with
# WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE, and then sets
# WS_VISIBLE | WS_POPUP, so IsWindowVisible is true for it and Process.MainWindowHandle may
# return it. WebView2's own windows are child windows or belong to msedgewebview2.exe.
#
# Get-ShownWindows -ProcessId <pid> returns objects with Class and Title (empty when none).
# Format-ShownWindows formats them for a log line.

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public sealed class VoicenShownWindow {
  public string Class;
  public string Title;
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
  const uint GW_OWNER = 4;
  const int GWL_EXSTYLE = -20;
  const int WS_EX_TOOLWINDOW = 0x80;
  const string TaoEventTarget = "Tao Thread Event Target";

  public static VoicenShownWindow[] Shown(uint pid) {
    var shown = new List<VoicenShownWindow>();
    EnumWindowsProc cb = (hWnd, lParam) => {
      uint owner;
      GetWindowThreadProcessId(hWnd, out owner);
      if (owner != pid || !IsWindowVisible(hWnd) || GetWindow(hWnd, GW_OWNER) != IntPtr.Zero) return true;
      if ((GetWindowLongW(hWnd, GWL_EXSTYLE) & WS_EX_TOOLWINDOW) != 0) return true;
      var cls = new StringBuilder(256);
      GetClassNameW(hWnd, cls, cls.Capacity);
      if (cls.ToString() == TaoEventTarget) return true;
      var title = new StringBuilder(512);
      GetWindowTextW(hWnd, title, title.Capacity);
      shown.Add(new VoicenShownWindow { Class = cls.ToString(), Title = title.ToString() });
      return true;
    };
    EnumWindows(cb, IntPtr.Zero);
    GC.KeepAlive(cb);
    return shown.ToArray();
  }
}
'@

function Get-ShownWindows([int]$ProcessId) { , [VoicenWindows]::Shown([uint32]$ProcessId) }

function Format-ShownWindows($Windows) {
  if (-not $Windows -or $Windows.Count -eq 0) { return 'none' }
  ($Windows | ForEach-Object { "class '$($_.Class)', title '$($_.Title)'" }) -join '; '
}
