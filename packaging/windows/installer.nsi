; The Windows installer, made with NSIS (makensis /DVERSION=1.2.3 installer.nsi)
; from the files `cargo build --release --features window` leaves in target\release.
; It installs for the current user, so it asks for no administrator rights, adds
; "Stream Spectre" to the Start menu and an entry to "Installed apps". The config
; and the saved settings are in %APPDATA%\stream-spectre and are kept on removal.

Unicode true
!include "MUI2.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif

Name "Stream Spectre"
OutFile "..\..\target\release\stream-spectre-${VERSION}-windows-x64-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\Stream Spectre"
InstallDirRegKey HKCU "Software\Stream Spectre" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma

!define MUI_ICON "..\..\assets\icon.ico"
!define MUI_UNICON "..\..\assets\icon.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\stream-spectre-window.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start Stream Spectre"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\..\LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Stream Spectre"
  SetOutPath "$INSTDIR"
  File "..\..\target\release\stream-spectre-window.exe"
  File "..\..\target\release\stream-spectre.exe"
  File "..\..\assets\icon.ico"
  File "..\..\LICENSE"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateShortcut "$SMPROGRAMS\Stream Spectre.lnk" "$INSTDIR\stream-spectre-window.exe" "" "$INSTDIR\icon.ico"

  WriteRegStr HKCU "Software\Stream Spectre" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Stream Spectre" "DisplayName" "Stream Spectre"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Stream Spectre" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Stream Spectre" "DisplayIcon" "$INSTDIR\icon.ico"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Stream Spectre" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Stream Spectre" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Stream Spectre" "NoRepair" 1
SectionEnd

Section "Uninstall"
  Delete "$SMPROGRAMS\Stream Spectre.lnk"
  Delete "$INSTDIR\stream-spectre-window.exe"
  Delete "$INSTDIR\stream-spectre.exe"
  Delete "$INSTDIR\icon.ico"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Stream Spectre"
  DeleteRegKey HKCU "Software\Stream Spectre"
SectionEnd
