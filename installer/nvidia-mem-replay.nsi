Unicode true
!include "MUI2.nsh"
!include "x64.nsh"
!include "FileFunc.nsh"
Name "nvidia-mem-replay"
OutFile "..\dist\nvidia-mem-replay-setup-x64.exe"
InstallDir "$PROGRAMFILES64\nvidia-mem-replay"
RequestExecutionLevel admin
SetCompressor /SOLID lzma
!define MUI_ABORTWARNING
!define MUI_ICON "..\assets\nvidia-mem-replay.ico"
!define MUI_UNICON "..\assets\nvidia-mem-replay.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\nvidia-mem-replay.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Launch nvidia-mem-replay now"
!define MUI_FINISHPAGE_RUN_FUNCTION LaunchApplication
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

# The installer is elevated; start the app as the logged-on user so it sees that user's
# NVIDIA overlay and tray.
Function LaunchApplication
  Exec '"$WINDIR\explorer.exe" "$INSTDIR\nvidia-mem-replay.exe"'
FunctionEnd

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "nvidia-mem-replay requires 64-bit Windows."
    Abort
  ${EndIf}

  SetRegView 64
  SetShellVarContext all
FunctionEnd

Section "nvidia-mem-replay"
  # Install the official signed WinFsp package, keeping compatible shared installs.
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File "..\dist\winfsp-2.1.25156.msi"

  SetRegView 32
  ReadRegStr $4 HKLM "Software\WinFsp" "InstallDir"
  SetRegView 64
  ${If} $4 != ""
    ClearErrors
    GetDLLVersion "$4\bin\winfsp-x64.dll" $1 $2
    ${IfNot} ${Errors}
      ${If} $1 >= 0x00020001
        Goto driver_ready
      ${EndIf}
    ${EndIf}
  ${EndIf}

  ExecWait '$SYSDIR\msiexec.exe /i "$PLUGINSDIR\winfsp-2.1.25156.msi" /passive /norestart ADDLOCAL=F.Main,F.User' $0
  ${If} $0 == 3010
    SetRebootFlag true
  ${ElseIf} $0 == 1641
    SetRebootFlag true
  ${ElseIf} $0 != 0
    MessageBox MB_ICONSTOP "WinFsp installation failed (code $0). Resolve the WinFsp installer error and run this installer again."
    Abort
  ${EndIf}

  driver_ready:
  SetOutPath "$INSTDIR"
  File "..\dist\nvidia-mem-replay.exe"
  File "..\dist\memefs-x64.exe"
  File "..\dist\README.md"
  File "..\dist\DEVELOPMENT.md"
  File "..\dist\LICENSE"

  SetOutPath "$INSTDIR\licenses"
  File "..\dist\licenses\*"

  SetOutPath "$INSTDIR\source"
  File "..\dist\source\*"
  SetOutPath "$INSTDIR"

  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateDirectory "$SMPROGRAMS\nvidia-mem-replay"
  CreateShortcut "$SMPROGRAMS\nvidia-mem-replay\nvidia-mem-replay.lnk" "$INSTDIR\nvidia-mem-replay.exe" "" "$INSTDIR\nvidia-mem-replay.exe" 0
  CreateShortcut "$SMPROGRAMS\nvidia-mem-replay\Uninstall.lnk" "$INSTDIR\uninstall.exe"

  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\NvidiaMemReplay" "DisplayIcon" "$INSTDIR\nvidia-mem-replay.exe"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\NvidiaMemReplay" "DisplayName" "nvidia-mem-replay"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\NvidiaMemReplay" "DisplayVersion" "0.1.0"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\NvidiaMemReplay" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\NvidiaMemReplay" "NoModify" 1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\NvidiaMemReplay" "NoRepair" 1
SectionEnd

Function un.onInit
  SetRegView 64
  SetShellVarContext all
  MessageBox MB_OKCANCEL "Quit nvidia-mem-replay from its tray menu before uninstalling, so NVIDIA's original temporary path is restored." IDOK ready
  Abort
  ready:
FunctionEnd

Section "Uninstall"
  # Leave shared WinFsp installed for other applications. Remove it separately
  # through Windows Apps and Features when it is no longer needed.
  Delete "$SMPROGRAMS\nvidia-mem-replay\nvidia-mem-replay.lnk"
  Delete "$SMPROGRAMS\nvidia-mem-replay\Uninstall.lnk"
  RMDir "$SMPROGRAMS\nvidia-mem-replay"

  Delete "$INSTDIR\nvidia-mem-replay.exe"
  Delete "$INSTDIR\memefs-x64.exe"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\DEVELOPMENT.md"
  Delete "$INSTDIR\LICENSE"
  RMDir /r "$INSTDIR\licenses"
  RMDir /r "$INSTDIR\source"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\NvidiaMemReplay"
SectionEnd
