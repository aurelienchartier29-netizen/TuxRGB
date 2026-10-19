; TuxRGB – Windows Beta Build installer (NSIS 3, Modern UI 2).
;
; Built by scripts/windows/build-windows.sh (from Linux) or
; scripts/windows/build-windows.ps1 (on Windows):
;   makensis -DVERSION=2.0.4-win-beta -DSTAGE=<dir> -DOUTFILE=<setup.exe> tuxrgb.nsi
;
; <STAGE> holds tuxrgb.exe, tuxrgb.ico, installer-side.bmp, fetch.ps1, LICENSE.txt,
; LIRE-MOI-Windows.txt and licenses\.
;
; OpenRGB and the PawnIO driver are downloaded from their official sources
; at install time (components that can be unticked).

Unicode true
SetCompressor /SOLID lzma
ManifestDPIAware true

!ifndef VERSION
  !define VERSION "2.0.4-win-beta"
!endif
!ifndef VERSION_NUM
  !define VERSION_NUM "2.0.4.0"
!endif
!ifndef STAGE
  !define STAGE "stage"
!endif
!ifndef OUTFILE
  !define OUTFILE "TuxRGB-${VERSION}-Setup.exe"
!endif

!define APP "TuxRGB"
!define PUBLISHER "TuxRGB contributors"
!define URL "https://github.com/aurelienchartier29-netizen/TuxRGB"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\TuxRGB"

Name "${APP} ${VERSION}"
Caption "${APP} ${VERSION} – Windows Beta Build"
BrandingText "${APP} ${VERSION} (Windows Beta Build)"
OutFile "${OUTFILE}"
InstallDir "$PROGRAMFILES64\${APP}"
InstallDirRegKey HKLM "${UNINST_KEY}" "InstallLocation"
RequestExecutionLevel admin
ShowInstDetails show
ShowUninstDetails show

VIProductVersion "${VERSION_NUM}"
VIAddVersionKey /LANG=0 "ProductName" "${APP}"
VIAddVersionKey /LANG=0 "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=0 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=0 "FileDescription" "${APP} – installateur (Windows Beta Build)"
VIAddVersionKey /LANG=0 "CompanyName" "${PUBLISHER}"
VIAddVersionKey /LANG=0 "LegalCopyright" "GPL-2.0-or-later"

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "x64.nsh"
!include "WinVer.nsh"
!include "FileFunc.nsh"
!include "Sections.nsh"

!define MUI_ICON "${STAGE}\tuxrgb.ico"
!define MUI_UNICON "${STAGE}\tuxrgb.ico"
!define MUI_WELCOMEFINISHPAGE_BITMAP "${STAGE}\installer-side.bmp"
!define MUI_UNWELCOMEFINISHPAGE_BITMAP "${STAGE}\installer-side.bmp"
!define MUI_ABORTWARNING
!define MUI_COMPONENTSPAGE_SMALLDESC

!define MUI_WELCOMEPAGE_TITLE "$(WelcomeTitle)"
!define MUI_WELCOMEPAGE_TEXT "$(WelcomeText)"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${STAGE}\LICENSE.txt"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "$(RunNow)"
!define MUI_FINISHPAGE_RUN_FUNCTION LaunchAsUser
!define MUI_FINISHPAGE_SHOWREADME "$INSTDIR\LIRE-MOI-Windows.txt"
!define MUI_FINISHPAGE_SHOWREADME_TEXT "$(ShowReadme)"
!define MUI_FINISHPAGE_SHOWREADME_NOTCHECKED
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "French"
!insertmacro MUI_LANGUAGE "English"

; ---------------------------------------------------------------------------
; Texts
; ---------------------------------------------------------------------------
LangString WelcomeTitle ${LANG_FRENCH} "Installation de TuxRGB ${VERSION}"
LangString WelcomeTitle ${LANG_ENGLISH} "TuxRGB ${VERSION} setup"
LangString WelcomeText ${LANG_FRENCH} "TuxRGB pilote tout l'éclairage RGB de votre PC (carte mère, RAM, carte graphique, ventilateurs, claviers, souris, bandes LED) avec le moteur OpenRGB.$\r$\n$\r$\nCeci est la Windows Beta Build. Linux reste la version de référence ; la build Windows est maintenue en priorité pour les correctifs de bugs majeurs.$\r$\n$\r$\nUne connexion Internet est utilisée pour télécharger OpenRGB 1.1 et le pilote PawnIO depuis leurs sites officiels."
LangString WelcomeText ${LANG_ENGLISH} "TuxRGB drives all the RGB lighting of your PC (motherboard, RAM, graphics card, fans, keyboards, mice, LED strips) with the OpenRGB engine.$\r$\n$\r$\nThis is the Windows Beta Build. Linux remains the reference platform; the Windows build is maintained first and foremost for major bug fixes.$\r$\n$\r$\nAn Internet connection is used to download OpenRGB 1.1 and the PawnIO driver from their official sources."
LangString RunNow ${LANG_FRENCH} "Lancer TuxRGB"
LangString RunNow ${LANG_ENGLISH} "Start TuxRGB"
LangString ShowReadme ${LANG_FRENCH} "Afficher les notes de la version bêta"
LangString ShowReadme ${LANG_ENGLISH} "Show the beta release notes"
LangString NeedWin10 ${LANG_FRENCH} "TuxRGB nécessite Windows 10 ou 11 en 64 bits."
LangString NeedWin10 ${LANG_ENGLISH} "TuxRGB requires 64-bit Windows 10 or 11."
LangString Running ${LANG_FRENCH} "TuxRGB est en cours d'exécution : il va être fermé pour continuer."
LangString Running ${LANG_ENGLISH} "TuxRGB is running: it will be closed to continue."
LangString OpenRGBFailed ${LANG_FRENCH} "OpenRGB 1.1 n'a pas pu être téléchargé (connexion Internet ?).$\r$\n$\r$\nTuxRGB utilisera une installation d'OpenRGB existante (C:\Program Files\OpenRGB) s'il en trouve une. Sinon, téléchargez OpenRGB 1.1 pour Windows sur openrgb.org et décompressez-le dans :$\r$\n$INSTDIR\openrgb"
LangString OpenRGBFailed ${LANG_ENGLISH} "OpenRGB 1.1 could not be downloaded (Internet connection?).$\r$\n$\r$\nTuxRGB will use an existing OpenRGB installation (C:\Program Files\OpenRGB) if it finds one. Otherwise download OpenRGB 1.1 for Windows from openrgb.org and unpack it into:$\r$\n$INSTDIR\openrgb"
LangString PawnIOFailed ${LANG_FRENCH} "Le pilote PawnIO n'a pas pu être installé. Les périphériques USB fonctionneront ; pour la carte mère et la RAM, installez PawnIO depuis pawnio.eu puis utilisez Réglages > Accès matériel > Configurer l'accès."
LangString PawnIOFailed ${LANG_ENGLISH} "The PawnIO driver could not be installed. USB devices will work; for the motherboard and the RAM, install PawnIO from pawnio.eu, then use Settings > Hardware access > Set up access."

LangString DescCore ${LANG_FRENCH} "L'application TuxRGB (interface, effets, profils, canevas spatial)."
LangString DescCore ${LANG_ENGLISH} "The TuxRGB application (interface, effects, profiles, spatial canvas)."
LangString DescOpenRGB ${LANG_FRENCH} "Moteur matériel OpenRGB 1.1 officiel, téléchargé depuis Codeberg / openrgb.org. Décochez pour utiliser votre propre installation d'OpenRGB."
LangString DescOpenRGB ${LANG_ENGLISH} "Official OpenRGB 1.1 hardware engine, downloaded from Codeberg / openrgb.org. Untick to use your own OpenRGB installation."
LangString DescPawnIO ${LANG_FRENCH} "Pilote PawnIO officiel : accès SMBus, indispensable pour la carte mère et la RAM."
LangString DescPawnIO ${LANG_ENGLISH} "Official PawnIO driver: SMBus access, needed for the motherboard and the RAM."
LangString DescAdmin ${LANG_FRENCH} "Lance TuxRGB avec les droits administrateur (SMBus) via une tâche planifiée, sans invite UAC à chaque démarrage."
LangString DescAdmin ${LANG_ENGLISH} "Starts TuxRGB with administrator rights (SMBus) through a scheduled task, without a UAC prompt at each start."
LangString DescDesktop ${LANG_FRENCH} "Raccourci TuxRGB sur le Bureau."
LangString DescDesktop ${LANG_ENGLISH} "TuxRGB shortcut on the desktop."

; ---------------------------------------------------------------------------
; Helpers
; ---------------------------------------------------------------------------
!macro CloseTuxRGB
  FindWindow $0 "TuxRGB.Host"
  ${If} $0 <> 0
    IfSilent +2
      MessageBox MB_OK|MB_ICONINFORMATION "$(Running)"
    nsExec::Exec '"$SYSDIR\taskkill.exe" /F /IM tuxrgb.exe'
    Pop $0
    Sleep 1500
  ${EndIf}
  ; The embedded OpenRGB server belongs to TuxRGB's job object: it stops
  ; with it (a separately installed OpenRGB is left alone).
!macroend

Function .onInit
  ${IfNot} ${RunningX64}
  ${OrIfNot} ${AtLeastWin10}
    MessageBox MB_OK|MB_ICONSTOP "$(NeedWin10)"
    Abort
  ${EndIf}
  SetRegView 64
FunctionEnd

; Started from the elevated installer, TuxRGB must still run with the
; user's normal rights (it hands over to administrator mode by itself).
Function LaunchAsUser
  Exec '"$WINDIR\explorer.exe" "$INSTDIR\tuxrgb.exe"'
FunctionEnd

; ---------------------------------------------------------------------------
; Sections
; ---------------------------------------------------------------------------
Section "TuxRGB" SecCore
  SectionIn RO
  !insertmacro CloseTuxRGB
  SetOutPath "$INSTDIR"
  File "${STAGE}\tuxrgb.exe"
  File "${STAGE}\tuxrgb.ico"
  File "${STAGE}\LICENSE.txt"
  File "${STAGE}\LIRE-MOI-Windows.txt"
  SetOutPath "$INSTDIR\licenses"
  File /r "${STAGE}\licenses\*.*"
  SetOutPath "$INSTDIR"

  WriteUninstaller "$INSTDIR\uninstall.exe"

  SetShellVarContext all
  CreateDirectory "$SMPROGRAMS\TuxRGB"
  CreateShortCut "$SMPROGRAMS\TuxRGB\TuxRGB.lnk" "$INSTDIR\tuxrgb.exe" "" "$INSTDIR\tuxrgb.ico" 0
  CreateShortCut "$SMPROGRAMS\TuxRGB\Désinstaller TuxRGB.lnk" "$INSTDIR\uninstall.exe" "" "$INSTDIR\tuxrgb.ico" 0

  WriteRegStr HKLM "${UNINST_KEY}" "DisplayName" "TuxRGB (Windows Beta Build)"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINST_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKLM "${UNINST_KEY}" "URLInfoAbout" "${URL}"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\tuxrgb.ico"
  WriteRegStr HKLM "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoRepair" 1
SectionEnd

Section "OpenRGB 1.1" SecOpenRGB
  InitPluginsDir
  File "/oname=$PLUGINSDIR\fetch.ps1" "${STAGE}\fetch.ps1"
  DetailPrint "OpenRGB 1.1…"
  nsExec::ExecToLog '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\fetch.ps1" -What openrgb -Dest "$INSTDIR\openrgb"'
  Pop $0
  ; The exit code alone is not trusted: the engine must really be there.
  ${If} $0 != 0
  ${OrIfNot} ${FileExists} "$INSTDIR\openrgb\OpenRGB.exe"
    DetailPrint "OpenRGB : échec ($0)"
    IfSilent +2
      MessageBox MB_OK|MB_ICONEXCLAMATION "$(OpenRGBFailed)"
  ${Else}
    DetailPrint "OpenRGB : installé dans $INSTDIR\openrgb"
  ${EndIf}
SectionEnd

Section "PawnIO (carte mère, RAM)" SecPawnIO
  InitPluginsDir
  File "/oname=$PLUGINSDIR\fetch.ps1" "${STAGE}\fetch.ps1"
  ReadRegStr $1 HKLM "SYSTEM\CurrentControlSet\Services\PawnIO" "ImagePath"
  ${If} $1 != ""
    DetailPrint "PawnIO : déjà installé"
  ${Else}
    DetailPrint "PawnIO…"
    nsExec::ExecToLog '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\fetch.ps1" -What pawnio -Dest "$INSTDIR\pawnio"'
    Pop $0
    ${If} $0 == 0
    ${AndIf} ${FileExists} "$INSTDIR\pawnio\PawnIO_setup.exe"
      ExecWait '"$INSTDIR\pawnio\PawnIO_setup.exe" -install -silent' $0
    ${EndIf}
    ReadRegStr $1 HKLM "SYSTEM\CurrentControlSet\Services\PawnIO" "ImagePath"
    ${If} $0 != 0
    ${OrIf} $1 == ""
      DetailPrint "PawnIO : échec ($0)"
      IfSilent +2
        MessageBox MB_OK|MB_ICONEXCLAMATION "$(PawnIOFailed)"
    ${EndIf}
  ${EndIf}
SectionEnd

Section "Mode administrateur" SecAdmin
  DetailPrint "Mode administrateur (tâche planifiée)…"
  ExecWait '"$INSTDIR\tuxrgb.exe" --windows-setup install "$TEMP\tuxrgb-install-report.txt"' $0
  DetailPrint "Mode administrateur : code $0"
  Delete "$TEMP\tuxrgb-install-report.txt"
SectionEnd

Section "Raccourci sur le Bureau" SecDesktop
  SetShellVarContext all
  CreateShortCut "$DESKTOP\TuxRGB.lnk" "$INSTDIR\tuxrgb.exe" "" "$INSTDIR\tuxrgb.ico" 0
SectionEnd

Section "-Size"
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKLM "${UNINST_KEY}" "EstimatedSize" "$0"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecCore} "$(DescCore)"
  !insertmacro MUI_DESCRIPTION_TEXT ${SecOpenRGB} "$(DescOpenRGB)"
  !insertmacro MUI_DESCRIPTION_TEXT ${SecPawnIO} "$(DescPawnIO)"
  !insertmacro MUI_DESCRIPTION_TEXT ${SecAdmin} "$(DescAdmin)"
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "$(DescDesktop)"
!insertmacro MUI_FUNCTION_DESCRIPTION_END

; ---------------------------------------------------------------------------
; Uninstaller
; ---------------------------------------------------------------------------
Function un.onInit
  SetRegView 64
FunctionEnd

Section "Uninstall"
  !insertmacro CloseTuxRGB
  ; Administrator mode of the user running the uninstaller.
  ExecWait '"$INSTDIR\tuxrgb.exe" --windows-setup reset "$TEMP\tuxrgb-uninstall-report.txt"' $0
  Delete "$TEMP\tuxrgb-uninstall-report.txt"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "TuxRGB"

  SetShellVarContext all
  Delete "$DESKTOP\TuxRGB.lnk"
  RMDir /r "$SMPROGRAMS\TuxRGB"

  RMDir /r "$INSTDIR\openrgb"
  RMDir /r "$INSTDIR\pawnio"
  RMDir /r "$INSTDIR\licenses"
  Delete "$INSTDIR\tuxrgb.exe"
  Delete "$INSTDIR\tuxrgb.ico"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\LIRE-MOI-Windows.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKLM "${UNINST_KEY}"
  ; The PawnIO driver stays installed (other tools use it); the settings
  ; stay in %APPDATA%\tuxrgb.
SectionEnd
