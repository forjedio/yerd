; Yerd NSIS installer hooks (Tauri `installerHooks`).
;
; Yerd installs per-user (installMode: currentUser) and registers NO service and
; NO Run key at install time: the app enables its own autostart at first run via
; yerd-service-ctl (a per-user HKCU Run entry). These hooks only:
;   - POSTINSTALL:  put `yerd` + the shim dir on the user PATH (idempotent).
;   - PREUNINSTALL: on a REAL uninstall only, tear down system/user state.
;
; DATA-SAFETY GUARD ($UpdateMode)
; -------------------------------
; Tauri's uninstaller runs on every upgrade / silent self-update (`/S /UPDATE`),
; not just a real uninstall. An unguarded `yerd uninstall --yes` here would wipe
; the NRPT rule, the CA, and the data dirs (and fire UAC) on every update. The
; generated `installer.nsi` (tauri CLI 2.11.2) parses `/UPDATE` into `$UpdateMode`
; in `un.onInit` BEFORE the Section Uninstall inserts NSIS_HOOK_PREUNINSTALL, so
; the `${If} $UpdateMode <> 1` guard below runs the destructive teardown ONLY on a
; genuine uninstall. Verified against the vendored template; do not remove it.

!macro NSIS_HOOK_POSTINSTALL
  ; Put `yerd` and the {data}\bin shim dir on the user PATH (per-user, no console
  ; window, idempotent). Registers no autostart - the app owns that.
  nsExec::ExecToStack '"$INSTDIR\yerd.exe" path install'
  Pop $0
  Pop $1
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Real uninstall only ($UpdateMode <> 1). On an upgrade / self-update this is
  ; skipped so user data, the CA, the NRPT rule and autostart all survive.
  ${If} $UpdateMode <> 1
    ; Full teardown: NRPT (one UAC), CA (CurrentUser store), the daemon Run value,
    ; PATH entries, data dirs; kills yerdd. Non-interactive under `--yes`.
    nsExec::ExecToStack '"$INSTDIR\yerd.exe" uninstall --yes'
    Pop $0
    Pop $1
    ; Belt-and-braces: remove both HKCU Run values in case the uninstall call
    ; above could not run. The Tauri template already deletes "Yerd" (the GUI
    ; autostart-plugin value) on a real uninstall; "Yerd Daemon" is Yerd's own
    ; daemon value, which the template does not know about.
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Yerd"
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Yerd Daemon"
  ${EndIf}
!macroend
