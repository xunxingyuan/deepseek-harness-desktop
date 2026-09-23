; DSH Desktop uses a uniquely named sidecar so Tauri's native process helper can
; close it without affecting unrelated Node.js applications.
!macro DSH_STOP_RUNTIME
  !insertmacro CheckIfAppIsRunning "dsh-runtime.exe" "DSH Desktop 后台运行时"
!macroend

; Builds before 0.1.14-test.3 shipped the sidecar as node.exe. Best-effort
; cleanup uses the full executable path, then schedules the obsolete binary for
; deletion after reboot if Windows still has it mapped. New builds never copy a
; node.exe file, so a legacy lock cannot block the upgrade.
!macro DSH_CLEAN_LEGACY_RUNTIME
  DetailPrint "Stopping legacy DSH Desktop runtime..."
  nsExec::ExecToStack `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object -Property Name -EQ -Value 'node.exe' | Where-Object -Property ExecutablePath -EQ -Value '$INSTDIR\node.exe' | Invoke-CimMethod -MethodName Terminate -ErrorAction SilentlyContinue | Out-Null"`
  Pop $R0
  Pop $R1
  Sleep 1000
  Delete /REBOOTOK "$INSTDIR\node.exe"
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; Closing the main process first prevents it from relaunching its sidecar.
  !insertmacro CheckIfAppIsRunning "${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  !insertmacro DSH_STOP_RUNTIME
  !insertmacro DSH_CLEAN_LEGACY_RUNTIME
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro CheckIfAppIsRunning "${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  !insertmacro DSH_STOP_RUNTIME
  !insertmacro DSH_CLEAN_LEGACY_RUNTIME
!macroend
