# Temporary workaround, RCA: T-066 (Windows premise not checked on Windows before main).
# T-061: Windows Credential Manager entries for the CI steps in .github/workflows/ci.yml,
# through the same Win32 calls and type the product uses (src-tauri/src/credentials.rs
# WinCredentialStore, src-tauri/src/win/purge.rs WinCredentialNamespace): CredWriteW,
# CredReadW, CredEnumerateW, CredDeleteW, CRED_TYPE_GENERIC, CRED_PERSIST_LOCAL_MACHINE.
# Dot-source it in a pwsh step (like scripts/ci/visible-windows.ps1). Windows only.
#
# Why (T-061 re-analysis after verify 3, run 37444780779): a step that planted and confirmed
# its premise through cmdkey and cmdkey's filtered /list text, with cmdkey's exit code and
# output discarded, failed with "could not plant" and could not say whether the write or the
# confirmation failed. A CI step decides "planted", "present" and "gone" only through these
# functions; no premise is read from another tool's text. make check (scripts/ci/ci-credentials.sh)
# refuses a Credential Manager entry point named anywhere else in the workflows or scripts/ci.
#
# Every failed Win32 call throws with the call, the target and the Win32 error (code and
# message). ERROR_NOT_FOUND (1168) is not a failure where the API uses it for "none":
#   Add-VoicenCredential -Target <t> -Secret <fake>  CredWriteW GENERIC, LOCAL_MACHINE, user 'voicen',
#                                                    the secret as UTF-8 bytes (like the product)
#   Test-VoicenCredential -Target <t>                CredReadW GENERIC: $true / $false (1168)
#   Get-VoicenCredentials -Prefix <p>                CredEnumerateW "<p>*" (the purge's own filter):
#                                                    objects with Target and Type, one per pipeline
#                                                    item (wrap in @(...)); none on 1168
#   Remove-VoicenCredential -Target <t>              CredDeleteW GENERIC; 1168 is fine (already gone)
# Blobs are never read back or returned: the secret only goes in.

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;

public sealed class VoicenCredentialEntry {
  public string Target;
  public uint Type;
}

public static class VoicenCredentials {
  // CREDENTIALW (wincred.h), x64 natural layout; strings and buffers as raw pointers.
  [StructLayout(LayoutKind.Sequential)]
  struct CREDENTIALW {
    public uint Flags;
    public uint Type;
    public IntPtr TargetName;
    public IntPtr Comment;
    public uint LastWrittenLow;
    public uint LastWrittenHigh;
    public uint CredentialBlobSize;
    public IntPtr CredentialBlob;
    public uint Persist;
    public uint AttributeCount;
    public IntPtr Attributes;
    public IntPtr TargetAlias;
    public IntPtr UserName;
  }

  [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
  static extern bool CredWriteW(ref CREDENTIALW credential, uint flags);
  [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
  static extern bool CredReadW(string target, uint type, uint flags, out IntPtr credential);
  [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
  static extern bool CredEnumerateW(string filter, uint flags, out uint count, out IntPtr credentials);
  [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
  static extern bool CredDeleteW(string target, uint type, uint flags);
  [DllImport("advapi32.dll")]
  static extern void CredFree(IntPtr buffer);

  public const uint CRED_TYPE_GENERIC = 1;
  const uint CRED_PERSIST_LOCAL_MACHINE = 2;
  public const int ERROR_NOT_FOUND = 1168;

  static Exception Failure(string call, string target, int code) {
    return new InvalidOperationException(String.Format("{0}({1}) failed: Win32 error {2} ({3})",
      call, target, code, new Win32Exception(code).Message));
  }

  public static void Write(string target, string user, string secret) {
    byte[] blob = Encoding.UTF8.GetBytes(secret);
    IntPtr name = Marshal.StringToCoTaskMemUni(target);
    IntPtr userName = Marshal.StringToCoTaskMemUni(user);
    IntPtr buffer = Marshal.AllocCoTaskMem(blob.Length);
    try {
      Marshal.Copy(blob, 0, buffer, blob.Length);
      var cred = new CREDENTIALW {
        Type = CRED_TYPE_GENERIC,
        TargetName = name,
        CredentialBlobSize = (uint)blob.Length,
        CredentialBlob = buffer,
        Persist = CRED_PERSIST_LOCAL_MACHINE,
        UserName = userName,
      };
      if (!CredWriteW(ref cred, 0)) throw Failure("CredWriteW GENERIC", target, Marshal.GetLastWin32Error());
    } finally {
      Marshal.FreeCoTaskMem(buffer);
      Marshal.FreeCoTaskMem(userName);
      Marshal.FreeCoTaskMem(name);
    }
  }

  public static bool Exists(string target) {
    IntPtr cred;
    if (CredReadW(target, CRED_TYPE_GENERIC, 0, out cred)) {
      CredFree(cred);
      return true;
    }
    int code = Marshal.GetLastWin32Error();
    if (code == ERROR_NOT_FOUND) return false;
    throw Failure("CredReadW GENERIC", target, code);
  }

  public static VoicenCredentialEntry[] List(string prefix) {
    string filter = prefix + "*";
    uint count;
    IntPtr creds;
    if (!CredEnumerateW(filter, 0, out count, out creds)) {
      int code = Marshal.GetLastWin32Error();
      if (code == ERROR_NOT_FOUND) return new VoicenCredentialEntry[0];
      throw Failure("CredEnumerateW", filter, code);
    }
    var found = new List<VoicenCredentialEntry>();
    try {
      for (int i = 0; i < (int)count; i++) {
        IntPtr p = Marshal.ReadIntPtr(creds, i * IntPtr.Size);
        if (p == IntPtr.Zero) continue;
        var c = (CREDENTIALW)Marshal.PtrToStructure(p, typeof(CREDENTIALW));
        string target = c.TargetName == IntPtr.Zero ? null : Marshal.PtrToStringUni(c.TargetName);
        found.Add(new VoicenCredentialEntry { Target = target, Type = c.Type });
      }
    } finally {
      CredFree(creds);
    }
    return found.ToArray();
  }

  public static void Delete(string target) {
    if (CredDeleteW(target, CRED_TYPE_GENERIC, 0)) return;
    int code = Marshal.GetLastWin32Error();
    if (code == ERROR_NOT_FOUND) return;
    throw Failure("CredDeleteW GENERIC", target, code);
  }
}
'@

function Add-VoicenCredential {
  param([Parameter(Mandatory = $true)][string]$Target, [Parameter(Mandatory = $true)][string]$Secret)
  [VoicenCredentials]::Write($Target, 'voicen', $Secret)
}

function Test-VoicenCredential {
  param([Parameter(Mandatory = $true)][string]$Target)
  [VoicenCredentials]::Exists($Target)
}

function Get-VoicenCredentials {
  param([Parameter(Mandatory = $true)][string]$Prefix)
  # No leading comma: the entries go to the pipeline one by one, so a caller's @(...) is a flat
  # array of entries for 0, 1 or many (T-061 review 5 #3).
  [VoicenCredentials]::List($Prefix)
}

function Remove-VoicenCredential {
  param([Parameter(Mandatory = $true)][string]$Target)
  [VoicenCredentials]::Delete($Target)
}
