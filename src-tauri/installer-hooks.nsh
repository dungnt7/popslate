; App data lives in ".popslate" folders (set in code, see config.rs), not in the identifier
; folders the generated uninstaller deletes. Remove them when the user ticks "delete app data".
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    RmDir /r "$APPDATA\.popslate"
    RmDir /r "$LOCALAPPDATA\.popslate"
  ${EndIf}
!macroend
