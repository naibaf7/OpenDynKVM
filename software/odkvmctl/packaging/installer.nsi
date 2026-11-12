; OpenDynKVM host software for Windows: a per-user installer, no administrator rights needed (software.md §9).
; Built by `cargo xtask package`, which stages the files and runs
;   makensis -DVERSION=<x.y.z> -DSRC=<staged files> -DOUTFILE=<installer.exe> installer.nsi
;
; It installs opendyn-kvm.exe (window and tray icon) and odkvmctl.exe (command line) to %LOCALAPPDATA%\Programs\OpenDynKVM,
; with the release's KVM firmware in firmware\, a Start menu entry and an "Apps" entry with an uninstaller. "Start with Windows" writes the same Run value as the
; GUI's own setting (odkvmctl::autostart), so either can switch it later.

Unicode true
!include "MUI2.nsh"
!include "FileFunc.nsh"

!define APP "OpenDynKVM"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP}"
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define APPROVED_KEY "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"

Name "${APP}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\${APP}"
InstallDirRegKey HKCU "Software\${APP}" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${APP}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "${APP} host software setup"
VIAddVersionKey "LegalCopyright" "Fabian Tschopp, MIT OR Apache-2.0"

!define MUI_ICON "${SRC}/opendyn-kvm.ico"
!define MUI_UNICON "${SRC}/opendyn-kvm.ico"
!define MUI_COMPONENTSPAGE_NODESC
!define MUI_FINISHPAGE_RUN "$INSTDIR\opendyn-kvm.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Open ${APP} now"

!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; A running copy (the tray icon) keeps opendyn-kvm.exe open.
!macro CLOSE_RUNNING
  nsExec::Exec '"$SYSDIR\taskkill.exe" /IM opendyn-kvm.exe /F'
  Pop $0
  Sleep 300
!macroend

Section "${APP}" SecMain
  SectionIn RO
  !insertmacro CLOSE_RUNNING
  SetOutPath "$INSTDIR"
  File "${SRC}/opendyn-kvm.exe"
  File "${SRC}/odkvmctl.exe"
  File "${SRC}/opendyn-kvm.ico"
  SetOutPath "$INSTDIR\licenses"
  File "${SRC}/licenses/*"
  ; The release's firmware, which the window and odkvmctl update take by default.
  SetOutPath "$INSTDIR\firmware"
  File "${SRC}/firmware/*"
  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortCut "$SMPROGRAMS\${APP}.lnk" "$INSTDIR\opendyn-kvm.exe" "" "$INSTDIR\opendyn-kvm.ico"

  WriteRegStr HKCU "Software\${APP}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${APP}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "${APP}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\opendyn-kvm.ico"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" "$0"

  ; Every install decides afresh: the next section adds it back if it stays ticked.
  DeleteRegValue HKCU "${RUN_KEY}" "${APP}"
SectionEnd

Section "Start with Windows, in the tray" SecAutostart
  WriteRegStr HKCU "${RUN_KEY}" "${APP}" '"$INSTDIR\opendyn-kvm.exe" --tray'
  ; A "disabled" left by Task Manager would keep it off.
  DeleteRegValue HKCU "${APPROVED_KEY}" "${APP}"
SectionEnd

Section "Uninstall"
  !insertmacro CLOSE_RUNNING
  DeleteRegValue HKCU "${RUN_KEY}" "${APP}"
  DeleteRegValue HKCU "${APPROVED_KEY}" "${APP}"
  Delete "$SMPROGRAMS\${APP}.lnk"
  Delete "$INSTDIR\opendyn-kvm.exe"
  Delete "$INSTDIR\odkvmctl.exe"
  Delete "$INSTDIR\opendyn-kvm.ico"
  RMDir /r "$INSTDIR\licenses"
  RMDir /r "$INSTDIR\firmware"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "Software\${APP}"
SectionEnd
