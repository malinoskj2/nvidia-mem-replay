Unicode true
!include "MUI2.nsh"
!include "x64.nsh"
!include "FileFunc.nsh"
Name "Replay in RAM"
OutFile "../dist/nvidia-capture-in-ram-x64.exe"
InstallDir "$PROGRAMFILES64\Replay in RAM"
RequestExecutionLevel admin
SetCompressor /SOLID lzma
!define MUI_ABORTWARNING
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "../LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "Replay in RAM requires 64-bit Windows."
    Abort
  ${EndIf}

  SetRegView 64
  SetShellVarContext all
FunctionEnd

Section "Replay in RAM"
  # Install the official signed WinFsp package, keeping compatible shared installs.
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File "../dist/winfsp-2.1.25156.msi"

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
  File "../dist/nvidia-mem-replay.exe"
  File "../dist/memefs-x64.exe"
  File "../dist/README.md"
  File "../dist/DEVELOPMENT.md"
  File "../dist/LICENSE"

  SetOutPath "$INSTDIR\licenses"
  File "../dist/licenses/*"

  SetOutPath "$INSTDIR\source"
  File "../dist/source/*"
  SetOutPath "$INSTDIR"

  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateDirectory "$SMPROGRAMS\Replay in RAM"
  CreateShortcut "$SMPROGRAMS\Replay in RAM\Replay in RAM.lnk" "$INSTDIR\nvidia-mem-replay.exe"
  CreateShortcut "$SMPROGRAMS\Replay in RAM\Uninstall.lnk" "$INSTDIR\uninstall.exe"

  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\ReplayInRam" "DisplayName" "Replay in RAM"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\ReplayInRam" "DisplayVersion" "0.1.0"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\ReplayInRam" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\ReplayInRam" "NoModify" 1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\ReplayInRam" "NoRepair" 1
SectionEnd

Function un.onInit
  SetRegView 64
  SetShellVarContext all
  MessageBox MB_OKCANCEL "Quit Replay in RAM from its tray menu before uninstalling, so NVIDIA's original temporary path is restored." IDOK ready
  Abort
  ready:
FunctionEnd

Section "Uninstall"
  # Leave shared WinFsp installed for other applications. Remove it separately
  # through Windows Apps and Features when it is no longer needed.
  Delete "$SMPROGRAMS\Replay in RAM\Replay in RAM.lnk"
  Delete "$SMPROGRAMS\Replay in RAM\Uninstall.lnk"
  RMDir "$SMPROGRAMS\Replay in RAM"

  Delete "$INSTDIR\nvidia-mem-replay.exe"
  Delete "$INSTDIR\memefs-x64.exe"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\DEVELOPMENT.md"
  Delete "$INSTDIR\LICENSE"
  RMDir /r "$INSTDIR\licenses"
  RMDir /r "$INSTDIR\source"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\ReplayInRam"
SectionEnd
