; Close only the Node.js sidecar installed with DSH Desktop. Matching the full
; executable path avoids terminating unrelated Node.js applications.
!macro DSH_STOP_BUNDLED_RUNTIME
  !define DshRuntimeUniqueID ${__LINE__}
  dsh_runtime_retry_${DshRuntimeUniqueID}:
    nsExec::ExecToStack `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "Get-Process -Name node -ErrorAction SilentlyContinue | Where-Object -Property Path -EQ -Value '$INSTDIR\node.exe' | Stop-Process -Force -ErrorAction Stop"`
    Pop $R0
    Pop $R1
    Sleep 750

    nsExec::ExecToStack `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "if (@(Get-Process -Name node -ErrorAction SilentlyContinue | Where-Object -Property Path -EQ -Value '$INSTDIR\node.exe').Count -gt 0) { exit 1 } else { exit 0 }"`
    Pop $R0
    Pop $R1
    ${If} $R0 != 0
      IfSilent dsh_runtime_abort_${DshRuntimeUniqueID} dsh_runtime_prompt_${DshRuntimeUniqueID}
      dsh_runtime_prompt_${DshRuntimeUniqueID}:
        MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "DSH Desktop 后台运行时仍在运行。请关闭 DSH Desktop 后点击重试。$\r$\n$\r$\nThe DSH Desktop background runtime is still running. Close DSH Desktop, then click Retry." IDRETRY dsh_runtime_retry_${DshRuntimeUniqueID} IDCANCEL dsh_runtime_abort_${DshRuntimeUniqueID}
      dsh_runtime_abort_${DshRuntimeUniqueID}:
        Abort "DSH Desktop background runtime is still running."
    ${EndIf}
  !undef DshRuntimeUniqueID
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; Tauri's normal check closes the app first so its shutdown path can stop the
  ; child cleanly. The path-scoped fallback handles an orphaned sidecar.
  !insertmacro CheckIfAppIsRunning "${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  !insertmacro DSH_STOP_BUNDLED_RUNTIME
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro CheckIfAppIsRunning "${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  !insertmacro DSH_STOP_BUNDLED_RUNTIME
!macroend
