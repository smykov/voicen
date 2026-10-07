# T-006 (Refresh 5): a real hold of the dictation hotkey against the installed release app, for
# the install smoke in .github/workflows/ci.yml (dot-sourced by the first-launch and the
# loaded-launch hold steps). Windows only; pwsh.
#
# Raw Win32 facts only (F-003): the hotkey is "held by the app" when this thread's own
# RegisterHotKey of Ctrl+Alt+Space (MOD_NOREPEAT) fails with 1409; the hold is one SendInput batch
# Ctrl down, Alt down, Space down, timed from the moment GetAsyncKeyState reads all three down (the
# observed press, F-005: injected input reached the system after ~1 s on run B), then one batch
# Space up, Alt up, Ctrl up. The key-ups are sent on every path (finally), so a failed step never
# leaves a modifier down for the steps after it. Runner facts behind this: `sendinput`, `hotkey`
# and `async_keys` are ok on windows-latest (docs/decisions/windows-ci-runner.md).
#
# Log lines are counted over voicen.log plus every rolled voicen-*.log (a roll at local midnight
# renames the file and keeps its lines, T-008), with Select-String -SimpleMatch (shared read).

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace T006Hold {
  [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
  [StructLayout(LayoutKind.Sequential)] public struct MOUSEINPUT { public int dx; public int dy; public uint mouseData; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
  [StructLayout(LayoutKind.Explicit)] public struct InputUnion { [FieldOffset(0)] public MOUSEINPUT mi; [FieldOffset(0)] public KEYBDINPUT ki; }
  [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public InputUnion u; }
  public static class Native {
    [DllImport("user32.dll", SetLastError = true)] public static extern uint SendInput(uint n, INPUT[] inputs, int size);
    [DllImport("user32.dll")] public static extern short GetAsyncKeyState(int vk);
    [DllImport("user32.dll", SetLastError = true)] public static extern bool RegisterHotKey(IntPtr hWnd, int id, uint fsModifiers, uint vk);
    [DllImport("user32.dll", SetLastError = true)] public static extern bool UnregisterHotKey(IntPtr hWnd, int id);
    const uint INPUT_KEYBOARD = 1;
    const uint KEYEVENTF_KEYUP = 0x0002;
    // Sends one batch of key events (vk, up); returns the inserted count, or -<Win32 error> when 0.
    public static int Keys(ushort[] vks, bool up) {
      INPUT[] batch = new INPUT[vks.Length];
      for (int i = 0; i < vks.Length; i++) {
        batch[i].type = INPUT_KEYBOARD;
        batch[i].u.ki.wVk = vks[i];
        batch[i].u.ki.dwFlags = up ? KEYEVENTF_KEYUP : 0;
      }
      uint n = SendInput((uint)batch.Length, batch, Marshal.SizeOf(typeof(INPUT)));
      if (n == 0) { return -Marshal.GetLastWin32Error(); }
      return (int)n;
    }
  }
}
'@

$VoicenHoldDown = [uint16[]]@(0x11, 0x12, 0x20)   # VK_CONTROL, VK_MENU, VK_SPACE
$VoicenHoldUp = [uint16[]]@(0x20, 0x12, 0x11)

# 0 when this thread could register Ctrl+Alt+Space (then it is freed at once), else the Win32 error
# (1409 = ERROR_HOTKEY_ALREADY_REGISTERED: another process, the app, holds it).
function Test-HotkeyFree {
  if ([T006Hold.Native]::RegisterHotKey([IntPtr]::Zero, 0x0A07, 0x4003, 0x20)) { [void][T006Hold.Native]::UnregisterHotKey([IntPtr]::Zero, 0x0A07); return 0 }
  return [Runtime.InteropServices.Marshal]::GetLastWin32Error()
}

# Waits up to $Seconds for the app to hold the hotkey; returns the last RegisterHotKey result
# (1409 when held). $Process: stops early when it exited.
function Wait-HotkeyHeld($Process, [int]$Seconds = 15) {
  $held = 0
  for ($i = 0; $i -lt ($Seconds * 2) -and $held -ne 1409; $i++) {
    if ($Process.HasExited) { break }
    $held = Test-HotkeyFree
    if ($held -ne 1409) { Start-Sleep -Milliseconds 500 }
  }
  $held
}

function Test-KeysDown([uint16[]]$Vks) {
  foreach ($vk in $Vks) { if (([T006Hold.Native]::GetAsyncKeyState([int]$vk) -band 0x8000) -eq 0) { return $false } }
  $true
}

function Test-KeysUp([uint16[]]$Vks) {
  foreach ($vk in $Vks) { if (([T006Hold.Native]::GetAsyncKeyState([int]$vk) -band 0x8000) -ne 0) { return $false } }
  $true
}

# Holds Ctrl+Alt+Space for $Seconds from the observed press. Returns the raw facts as a string
# (inserted counts, ms until the keys read down and up). Throws when the press was not inserted or
# never read down within 5 s; the key-ups are sent in every case.
function Invoke-HotkeyHold([int]$Seconds = 3) {
  $clock = [Diagnostics.Stopwatch]::StartNew()
  $down = 0; $up = 0; $downMs = -1; $upMs = -1
  try {
    $down = [T006Hold.Native]::Keys($VoicenHoldDown, $false)
    if ($down -ne 3) { throw "SendInput of Ctrl, Alt, Space down inserted $down of 3 (negative: -Win32 error)" }
    while ($clock.ElapsedMilliseconds -lt 5000 -and -not (Test-KeysDown $VoicenHoldDown)) { Start-Sleep -Milliseconds 10 }
    if (-not (Test-KeysDown $VoicenHoldDown)) { throw "Ctrl, Alt, Space did not read down within 5 s of SendInput (inserted $down)" }
    $downMs = $clock.ElapsedMilliseconds
    Start-Sleep -Seconds $Seconds
  } finally {
    $up = [T006Hold.Native]::Keys($VoicenHoldUp, $true)
    $upAt = $clock.ElapsedMilliseconds
    while ($clock.ElapsedMilliseconds - $upAt -lt 5000 -and -not (Test-KeysUp $VoicenHoldDown)) { Start-Sleep -Milliseconds 10 }
    if (Test-KeysUp $VoicenHoldDown) { $upMs = $clock.ElapsedMilliseconds - $upAt }
  }
  if ($up -ne 3) { throw "SendInput of Space, Alt, Ctrl up inserted $up of 3 (negative: -Win32 error)" }
  if ($upMs -lt 0) { throw "Ctrl, Alt, Space still read down 5 s after the key-ups" }
  "hold: down inserted=$down read_down_ms=$downMs held_s=$Seconds up inserted=$up read_up_ms=$upMs"
}

# The lines of every voicen*.log under $LogGlob that contain $Text (simple match).
function Get-VoicenLogLines([string]$LogGlob, [string]$Text) {
  if (-not (Test-Path $LogGlob)) { return , @() }
  , @(Select-String -Path $LogGlob -SimpleMatch $Text | ForEach-Object { $_.Line })
}

# Stops every voicen.exe (a running primary would take a launch through single instance) and
# throws when one is still there 10 s later.
function Stop-AllVoicen {
  Get-Process -Name voicen -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  for ($i = 0; $i -lt 20; $i++) {
    if (@(Get-Process -Name voicen -ErrorAction SilentlyContinue).Count -eq 0) { return }
    Start-Sleep -Milliseconds 500
  }
  throw "premise: a voicen process is still running 10 s after Stop-Process"
}
