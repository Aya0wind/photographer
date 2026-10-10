!macro NSIS_HOOK_POSTINSTALL
  ; Pure install option: ask when existing user data detected.
  ; YES = purge user data + library DBs (photos untouched); NO = keep data.
  ; POSTINSTALL because purge-user-data.ps1 is an app resource now present in $INSTDIR.
  ; 标识符三代：photographer ← photographer ← com.smartphoto.app，任一存在即算已有数据。
  IfFileExists "$APPDATA\photographer\settings.json" phub_ask_purge
  IfFileExists "$APPDATA\photographer\settings.json" phub_ask_purge
  IfFileExists "$APPDATA\com.smartphoto.app\settings.json" phub_ask_purge phub_no_purge
  phub_ask_purge:
    MessageBox MB_YESNO|MB_ICONQUESTION "检测到已有的 Photographer 用户数据。$\n$\n是 — 纯净安装：删除用户数据与图库数据库（照片文件不受影响）$\n否 — 保留全部数据，直接升级覆盖" IDYES phub_do_purge
    Goto phub_no_purge
  phub_do_purge:
    DetailPrint "纯净安装：清除用户数据与图库数据库（照片不受影响）..."
    IfFileExists "$INSTDIR\purge-user-data.ps1" 0 phub_no_purge
    nsExec::ExecToLog 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\purge-user-data.ps1"'
  phub_no_purge:
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Ask to remove user data on uninstall (PREUNSTALL: app files still present).
  ; Same semantics as pure install: never touch photo roots.
  IfFileExists "$APPDATA\photographer\settings.json" phub_ask_udata
  IfFileExists "$APPDATA\photographer\settings.json" phub_ask_udata
  IfFileExists "$APPDATA\com.smartphoto.app\settings.json" phub_ask_udata phub_no_udata
  phub_ask_udata:
    MessageBox MB_YESNO|MB_ICONQUESTION "是否同时删除用户数据与图库数据库？$\n$\n照片文件不会被删除。" IDYES phub_do_udata
    Goto phub_no_udata
  phub_do_udata:
    DetailPrint "删除用户数据与图库数据库（照片不受影响）..."
    IfFileExists "$INSTDIR\purge-user-data.ps1" 0 phub_no_udata
    nsExec::ExecToLog 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\purge-user-data.ps1"'
  phub_no_udata:
!macroend
