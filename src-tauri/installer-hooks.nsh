; The app writes its own "start with Windows" entry (tauri-plugin-autostart), which the
; generated uninstaller does not know about. Remove it so no entry points to a deleted exe.
!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Popslate"
!macroend
