; Voicen installer hooks (T-025; FR-28 v4, spec 006 FR-021..024, decisions #10, #76, OQ-19).
; Included by the forked template src-tauri/windows/installer.nsi (bundle.windows.nsis.installerHooks).
;
; Invariant (docs/decisions/installer.md): user data, %LOCALAPPDATA%\Voicen and every Credential
; Manager entry whose target starts with voicen_core::secrets::CREDENTIAL_TARGET_PREFIX ("Voicen/"),
; is removed at exactly one decision point, NSIS_HOOK_PREUNINSTALL below:
;   - started by an installer: the fork passes /UPDATE /P ($UpdateMode = 1) -> keep, ask nothing;
;   - /KEEPDATA -> keep;
;   - /S, or /P started by the user (OQ-19 (a)) -> remove (the default answer Yes);
;   - interactive -> ask, default button Yes.
; The Run value and the shortcuts are left to the stock template (removed unless /UPDATE).
;
; This file is included before MUI_LANGUAGE, so ${LANG_ENGLISH} / ${LANG_RUSSIAN} are not defined
; yet: the LangStrings use the numeric LANGIDs (1033 English, 1049 Russian).

LangString VoicenRemoveData 1033 "Remove settings, history, models, logs and saved keys?"
LangString VoicenRemoveData 1049 "Удалить настройки, историю, модели, журналы и сохранённые ключи?"
LangString VoicenKeysLeft 1033 "Saved keys could not be removed. Remove the entries starting with Voicen/ in Windows Credential Manager."
LangString VoicenKeysLeft 1049 "Не удалось удалить сохранённые ключи. Удалите в диспетчере учётных данных Windows записи, начинающиеся с Voicen/."
LangString VoicenFilesLeft 1033 "Some files in %LOCALAPPDATA%\Voicen could not be removed."
LangString VoicenFilesLeft 1049 "Не удалось удалить некоторые файлы в %LOCALAPPDATA%\Voicen."

; 1 = remove user data in this uninstall, decided once in NSIS_HOOK_PREUNINSTALL.
Var VoicenPurgeData
; Scratch for /KEEPDATA and the purge exit code.
Var VoicenScratch

; Every install records its language, silent ones included. The uninstaller's un.onInit runs
; MUI_UNGETLANGUAGE (installer.nsi): with no "Installer Language" value under
; MUI_LANGDLL_REGISTRY_KEY it calls MUI_LANGDLL_DISPLAY, which is skipped only under /S, and
; LangDLL shows its "Installer Language" dialog whenever more than one language is loaded
; (English + Russian). MUI writes the value only from the instfiles page's leave function, which
; never runs in a silent install, so without this line the first uninstall started by a passive
; reinstall (/UPDATE /P) or a user's /P waits on that dialog (T-025, CI run 37765499554).
!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr ${MUI_LANGDLL_REGISTRY_ROOT} "${MUI_LANGDLL_REGISTRY_KEY}" "${MUI_LANGDLL_REGISTRY_VALUENAME}" $LANGUAGE
!macroend

; The installer-ended note (T-024; invariant 5, docs/decisions/diagnostics-log.md): written to
; %LOCALAPPDATA%\Voicen\logs\installer-ended only while voicen.exe is running, so the stock
; CheckIfAppIsRunning that follows the PRE hooks is what ends that session. Restart Manager
; normally closes the app through WM_QUERYENDSESSION/WM_ENDSESSION, a clean end (marker deleted,
; `exited` line), and the next start's Session::begin only consumes the note (CI run 39009159665).
; Only when Restart Manager falls back to a forced kill does the marker survive, and then the note
; (mtime >= marker mtime) makes it ended_by_installer instead of ended_abnormally. With no running
; voicen.exe nothing is written, so an earlier kill stays ended_abnormally. "Running" is asked the
; way CheckIfAppIsRunning asks it (Restart Manager, RmGetList on "$INSTDIR\voicen.exe":
; ERROR_MORE_DATA = some process has it open). The content is not read. An empty $LOCALAPPDATA or
; a missing logs dir writes nothing. Clobbers $R0 and $0..$3, which CheckIfAppIsRunning sets
; again right after.
!macro VoicenNoteInstallerEnded
  !insertmacro RestartManager_StartSession $R0
  ${If} $R0 != ""
    !insertmacro RestartManager_RegisterFile $R0 "$INSTDIR\${MAINBINARYNAME}.exe"
    ${If} $0 = 0
      System::Call 'RSTRTMGR::RmGetList(p R0, *i .r1, *i .r2, p 0, *i .r3) i .r0'
      ${If} $0 = ${ERROR_MORE_DATA}
        SetShellVarContext current
        ${If} $LOCALAPPDATA != ""
        ${AndIf} ${FileExists} "$LOCALAPPDATA\Voicen\logs\*.*"
          ClearErrors
          FileOpen $1 "$LOCALAPPDATA\Voicen\logs\installer-ended" w
          ${IfNot} ${Errors}
            FileWrite $1 "installer-ended$\n"
            FileClose $1
          ${EndIf}
        ${EndIf}
      ${EndIf}
    ${EndIf}
    !insertmacro RestartManager_EndSession $R0
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro VoicenNoteInstallerEnded
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Any mode, before the purge: a reinstall's uninstaller closes the app first (decision #76),
  ; so the new installer's PREINSTALL would no longer see it running.
  !insertmacro VoicenNoteInstallerEnded

  StrCpy $VoicenPurgeData 0
  ${If} $UpdateMode <> 1
    ClearErrors
    ${GetOptions} $CMDLINE "/KEEPDATA" $VoicenScratch
    ${If} ${Errors}
      ${If} ${Silent}
      ${OrIf} $PassiveMode = 1
        StrCpy $VoicenPurgeData 1
      ${Else}
        MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON1 "$(VoicenRemoveData)" /SD IDYES IDNO +2
        StrCpy $VoicenPurgeData 1
      ${EndIf}
    ${EndIf}
  ${EndIf}

  ; The credentials go first, while voicen.exe still exists (the stock section deletes it next).
  ; voicen.exe --purge-credentials (T-061): exit 0 = every "Voicen/" entry removed or none,
  ; 2 = at least one could not be removed; no window, no log. A missing voicen.exe is not skipped:
  ; ExecWait then sets the error flag and the user gets the manual-removal message (FR-022).
  ${If} $VoicenPurgeData = 1
    ClearErrors
    StrCpy $VoicenScratch ""
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --purge-credentials' $VoicenScratch
    ${If} ${Errors}
    ${OrIf} $VoicenScratch <> 0
      ${If} $PassiveMode = 1
        DetailPrint "$(VoicenKeysLeft)"
      ${Else}
        MessageBox MB_OK|MB_ICONEXCLAMATION "$(VoicenKeysLeft)" /SD IDOK
      ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $VoicenPurgeData = 1
    ; Installer bookkeeping (install location, "Installer Language"), which the stock checkbox
    ; block removed before the fork dropped it (installer.nsi edit 2).
    DeleteRegKey SHCTX "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty SHCTX "${MANUKEY}"
    DeleteRegValue HKCU "${MANUPRODUCTKEY}" "Installer Language"
    DeleteRegKey /ifempty HKCU "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty HKCU "${MANUKEY}"

    ; The data folder is the literal %LOCALAPPDATA%\Voicen (paths::data_dir), not $INSTDIR: the
    ; interactive directory page may have put the program elsewhere. An empty $LOCALAPPDATA
    ; would turn the path into "\Voicen", so nothing is removed then.
    SetShellVarContext current
    ${If} $LOCALAPPDATA != ""
      RMDir /r "$LOCALAPPDATA\Voicen"
      ${If} ${FileExists} "$LOCALAPPDATA\Voicen\*.*"
        ${If} $PassiveMode = 1
          DetailPrint "$(VoicenFilesLeft)"
        ${Else}
          MessageBox MB_OK|MB_ICONEXCLAMATION "$(VoicenFilesLeft)" /SD IDOK
        ${EndIf}
      ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend
