; Skykeep's six apps in the Start menu beside Skykeep itself, as Linux has them in its launcher
; (packaging/apps): "Skykeep Notes" runs `skykeep --app notes`, and so on — named with Skykeep
; so typing "Notes" finds it and no other program's "Notes.lnk" is ever written over.
;
; Each shortcut carries its app's AppUserModelID, `<identifier>.Notes`, the one that app's
; window says once open (src-tauri/src/taskbar.rs): Windows gives each app its own taskbar
; button, and pins it as this shortcut. The "Skykeep" shortcut is Tauri's own, stamped with the
; identifier.
;
; Included by Tauri's installer.nsi (tauri.conf.json, bundle > windows > nsis > installerHooks);
; the MSI makes the same six from app-shortcuts.wxs. Every macro here is expanded inside that
; script, so its defines (PRODUCTNAME, BUNDLEID, MAINBINARYNAME), variables and COM helpers
; are there to use.

; A shortcut's AppUserModelID: Tauri's SetLnkAppUserModelId, for an ID other than BUNDLEID.
!macro SKYKEEP_LNK_APP_ID shortcut id
  !insertmacro ComHlpr_CreateInProcInstance ${CLSID_ShellLink} ${IID_IShellLink} r0 ""
  ${If} $0 P<> 0
    ${IUnknown::QueryInterface} $0 '("${IID_IPersistFile}",.r1)'
    ${If} $1 P<> 0
      ${IPersistFile::Load} $1 '("${shortcut}", ${STGM_READWRITE})'
      ${IUnknown::QueryInterface} $0 '("${IID_IPropertyStore}",.r2)'
      ${If} $2 P<> 0
        System::Call 'Oleaut32::SysAllocString(w "${id}") i.r3'
        System::Call '*${SYSSTRUCT_PROPERTYKEY}(${PKEY_AppUserModel_ID})p.r4'
        System::Call '*${SYSSTRUCT_PROPVARIANT}(${VT_BSTR},,&i4 $3)p.r5'
        ${IPropertyStore::SetValue} $2 '($4,$5)'
        System::Call 'Oleaut32::SysFreeString($3)'
        System::Free $4
        System::Free $5
        ${IPropertyStore::Commit} $2 ""
        ${IUnknown::Release} $2 ""
        ${IPersistFile::Save} $1 '("${shortcut}",1)'
      ${EndIf}
      ${IUnknown::Release} $1 ""
    ${EndIf}
    ${IUnknown::Release} $0 ""
  ${EndIf}
!macroend

; The folder Tauri puts "Skykeep" in, into $R9: the Start menu itself, unless a folder is
; configured. Decided where the macro is used: this file is included before STARTMENUFOLDER is
; defined.
!macro SKYKEEP_MENU
  !if "${STARTMENUFOLDER}" != ""
    StrCpy $R9 "$SMPROGRAMS\$AppStartMenuFolder"
  !else
    StrCpy $R9 "$SMPROGRAMS"
  !endif
!macroend

; One app's shortcut, in $R9: "Skykeep Notes", `skykeep --app notes`, Skykeep's icon, its own ID.
!macro SKYKEEP_APP_SHORTCUT name arg comment
  CreateShortcut "$R9\${PRODUCTNAME} ${name}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe" "--app ${arg}" "$INSTDIR\${MAINBINARYNAME}.exe" 0 SW_SHOWNORMAL "" "${comment}"
  !insertmacro SKYKEEP_LNK_APP_ID "$R9\${PRODUCTNAME} ${name}.lnk" "${BUNDLEID}.${name}"
!macroend

; One app's shortcut gone — unpinned first, and only if it is Skykeep's — with its jump list.
!macro SKYKEEP_APP_SHORTCUT_GONE name
  !insertmacro IsShortcutTarget "$R9\${PRODUCTNAME} ${name}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  Pop $0
  ${If} $0 = 1
    !insertmacro UnpinShortcut "$R9\${PRODUCTNAME} ${name}.lnk"
    Delete "$R9\${PRODUCTNAME} ${name}.lnk"
  ${EndIf}
  !insertmacro ComHlpr_CreateInProcInstance ${CLSID_DestinationList} ${IID_ICustomDestinationList} r1 ""
  ${If} $1 P<> 0
    ${ICustomDestinationList::DeleteList} $1 '("${BUNDLEID}.${name}")'
    ${IUnknown::Release} $1 ""
  ${EndIf}
!macroend

; After Tauri's own shortcuts, and on the same terms: not on an update or with /NS, but always
; when moving over from the MSI.
!macro NSIS_HOOK_POSTINSTALL
  ${If} $WixMode = 1
  ${OrIf} $UpdateMode <> 1
    ${If} $NoShortcutMode <> 1
    ${OrIf} $WixMode = 1
      !insertmacro SKYKEEP_MENU
      CreateDirectory "$R9"
      ; "Skykeep" itself stamped with the identifier too, as the process says it (taskbar.rs):
      ; toasts and the main window are attributed to that entry, whatever Tauri's template did.
      ${If} ${FileExists} "$R9\${PRODUCTNAME}.lnk"
        !insertmacro SKYKEEP_LNK_APP_ID "$R9\${PRODUCTNAME}.lnk" "${BUNDLEID}"
      ${EndIf}
      !insertmacro SKYKEEP_APP_SHORTCUT "Notes" "notes" "Notes from iCloud, on this computer"
      !insertmacro SKYKEEP_APP_SHORTCUT "Reminders" "reminders" "Reminders from iCloud, on this computer"
      !insertmacro SKYKEEP_APP_SHORTCUT "Calendar" "calendar" "Your iCloud calendars, on this computer"
      !insertmacro SKYKEEP_APP_SHORTCUT "Contacts" "contacts" "Your iCloud contacts, on this computer"
      !insertmacro SKYKEEP_APP_SHORTCUT "Drive" "drive" "iCloud Drive, on this computer"
      !insertmacro SKYKEEP_APP_SHORTCUT "Photos" "photos" "iCloud Photos, on this computer"
    ${EndIf}
  ${EndIf}
!macroend

; After Tauri has taken "Skykeep" away, and like it, kept on an update.
; iCloud Drive in File Explorer gone with Skykeep: no longer a cloud folder in Explorer's
; navigation pane (Nextcloud's stayed registered after uninstall, #3562). Before the program
; is removed, since it is what unregisters it; never on an update, which keeps the folder.
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --forget-cloud-folders'
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    !insertmacro SKYKEEP_MENU
    !insertmacro SKYKEEP_APP_SHORTCUT_GONE "Notes"
    !insertmacro SKYKEEP_APP_SHORTCUT_GONE "Reminders"
    !insertmacro SKYKEEP_APP_SHORTCUT_GONE "Calendar"
    !insertmacro SKYKEEP_APP_SHORTCUT_GONE "Contacts"
    !insertmacro SKYKEEP_APP_SHORTCUT_GONE "Drive"
    !insertmacro SKYKEEP_APP_SHORTCUT_GONE "Photos"
    !if "${STARTMENUFOLDER}" != ""
      RMDir "$R9"
    !endif
  ${EndIf}
!macroend
