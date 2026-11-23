; TianShell Windows Installer Script (NSIS 3.x)
; --------------------------------------------------
Unicode True
SetCompressor /SOLID lzma

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

; --- 基础信息定义 ---
!define PRODUCT_NAME "TianShell"
!define PRODUCT_DISPLAY_NAME "TianShell · 全功能终端工作台"
!define PRODUCT_VERSION "0.0.1"
!define PRODUCT_PUBLISHER "TianShell"
!define PRODUCT_WEB_SITE "https://github.com/zhangqi77ok-sys/tianshell"
!define PRODUCT_EXE "tianshell.exe"
!define PRODUCT_MCP_EXE "tianshell-mcp.exe"
!define PRODUCT_ICON "..\rust\assets\icon.ico"

; 运行权限：标准用户权限（免 UAC 烦扰，默认安装至 LocalAppData）
RequestExecutionLevel user

; 输出安装包文件名
OutFile "..\target\TianShell-Setup-${PRODUCT_VERSION}.exe"

; 默认安装路径与历史路径自动检测
InstallDir "$LOCALAPPDATA\Programs\TianShell"
InstallDirRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "InstallLocation"

; --- UI 视觉配置 ---
!define MUI_ICON "${PRODUCT_ICON}"
!define MUI_UNICON "${PRODUCT_ICON}"
!define MUI_ABORTWARNING

; 欢迎页面
!define MUI_WELCOMEPAGE_TITLE "欢迎使用 ${PRODUCT_DISPLAY_NAME} 安装向导"
!define MUI_WELCOMEPAGE_TEXT "本向导将指引您完成 ${PRODUCT_NAME} v${PRODUCT_VERSION} 的安装与配置。$\r$\n$\r$\n如果您已安装过旧版本，安装向导将自动识别并更新至原安装路径，且 100% 保留您的所有主机连接与凭据数据。$\r$\n$\r$\n点击【下一步】继续。"
!insertmacro MUI_PAGE_WELCOME

; 路径选择页面
!define MUI_DIRECTORYPAGE_TEXT_TOP "您可以自定义 TianShell 的安装路径。若为版本更新，已自动预选原有安装目录："
!insertmacro MUI_PAGE_DIRECTORY

; 安装进度页面
!insertmacro MUI_PAGE_INSTFILES

; 完成页面
!define MUI_FINISHPAGE_RUN "$INSTDIR\${PRODUCT_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "立即运行 ${PRODUCT_NAME}"
!insertmacro MUI_PAGE_FINISH

; --- 卸载页面 ---
!define MUI_WELCOMEPAGE_TITLE "卸载 ${PRODUCT_NAME}"
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH

; 语言支持
!insertmacro MUI_LANGUAGE "SimpChinese"

; --- 运行检测函数 ---
Function CheckRunningProcess
    ClearErrors
    ExecWait 'cmd /c "tasklist /FI \"IMAGENAME eq ${PRODUCT_EXE}\" 2>NUL | find /I \"${PRODUCT_EXE}\" >NUL"' $0
    ${If} $0 == 0
        ${If} ${Silent}
            ExecWait 'taskkill /F /IM ${PRODUCT_EXE} /T' $0
            Sleep 1000
            Goto Done
        ${Else}
            MessageBox MB_OKCANCEL|MB_ICONEXCLAMATION "检测到 ${PRODUCT_NAME} 正在运行。$\r$\n$\r$\n点击【确定】尝试自动关闭并继续更新/安装；$\r$\n点击【取消】退出向导。" IDOK CloseProc IDCANCEL AbortInstall
            CloseProc:
                ExecWait 'taskkill /F /IM ${PRODUCT_EXE} /T' $0
                Sleep 1000
                Goto Done
            AbortInstall:
                Abort
        ${EndIf}
    ${EndIf}
    Done:
FunctionEnd

Function un.CheckRunningProcess
    ClearErrors
    ExecWait 'cmd /c "tasklist /FI \"IMAGENAME eq ${PRODUCT_EXE}\" 2>NUL | find /I \"${PRODUCT_EXE}\" >NUL"' $0
    ${If} $0 == 0
        ${If} ${Silent}
            ExecWait 'taskkill /F /IM ${PRODUCT_EXE} /T' $0
            Sleep 1000
            Goto Done
        ${Else}
            MessageBox MB_OKCANCEL|MB_ICONEXCLAMATION "检测到 ${PRODUCT_NAME} 正在运行。$\r$\n$\r$\n点击【确定】关闭并继续卸载；$\r$\n点击【取消】退出卸载向导。" IDOK CloseProc IDCANCEL AbortUninstall
            CloseProc:
                ExecWait 'taskkill /F /IM ${PRODUCT_EXE} /T' $0
                Sleep 1000
                Goto Done
            AbortUninstall:
                Abort
        ${EndIf}
    ${EndIf}
    Done:
FunctionEnd

; --- 安装区段 ---
Section "MainSection" SEC01
    Call CheckRunningProcess

    SetOutPath "$INSTDIR"
    SetOverwrite on

    ; 复制核心二进制与静态资源
    File "..\target\release\${PRODUCT_EXE}"
    File "..\target\release\${PRODUCT_MCP_EXE}"
    File "..\target\release\WebView2Loader.dll"
    File "/oname=icon.ico" "${PRODUCT_ICON}"

    ; 复制前端 UI 资源
    SetOutPath "$INSTDIR\ui"
    File /r "..\ui\*.*"

    ; 回到安装根目录并生成独立卸载程序
    SetOutPath "$INSTDIR"
    WriteUninstaller "$INSTDIR\uninstall.exe"

    ; 创建开始菜单快捷方式
    CreateDirectory "$SMPROGRAMS\${PRODUCT_NAME}"
    CreateShortcut "$SMPROGRAMS\${PRODUCT_NAME}\${PRODUCT_NAME}.lnk" "$INSTDIR\${PRODUCT_EXE}" "" "$INSTDIR\icon.ico" 0 SW_SHOWNORMAL "" "${PRODUCT_DISPLAY_NAME}"
    CreateShortcut "$SMPROGRAMS\${PRODUCT_NAME}\卸载 ${PRODUCT_NAME}.lnk" "$INSTDIR\uninstall.exe" "" "$INSTDIR\uninstall.exe" 0 SW_SHOWNORMAL "" "卸载 ${PRODUCT_NAME}"

    ; 创建桌面快捷方式
    CreateShortcut "$DESKTOP\${PRODUCT_NAME}.lnk" "$INSTDIR\${PRODUCT_EXE}" "" "$INSTDIR\icon.ico" 0 SW_SHOWNORMAL "" "${PRODUCT_DISPLAY_NAME}"

    ; 注册到 Windows 添加/删除程序
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "DisplayName" "${PRODUCT_DISPLAY_NAME}"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "DisplayVersion" "${PRODUCT_VERSION}"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "Publisher" "${PRODUCT_PUBLISHER}"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "URLInfoAbout" "${PRODUCT_WEB_SITE}"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "InstallLocation" "$INSTDIR"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "UninstallString" '"$INSTDIR\uninstall.exe"'
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "DisplayIcon" "$INSTDIR\icon.ico"
    WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "NoModify" 1
    WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "NoRepair" 1

    ; 计算估算占用大小 (KB)
    ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
    IntFmt $0 "0x%08X" $0
    WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "EstimatedSize" "$0"
SectionEnd

; --- 卸载区段 ---
Section "Uninstall"
    Call un.CheckRunningProcess

    ; 静默模式默认保留配置；交互模式弹窗确认
    ${If} ${Silent}
        Goto KeepUserData
    ${Else}
        MessageBox MB_YESNO|MB_ICONQUESTION "是否保留您保存的主机连接、分组与终端配置数据？$\r$\n$\r$\n【是】保留配置数据（下次安装后可直接沿用，推荐）；$\r$\n【否】彻底清理所有配置数据。" IDYES KeepUserData IDNO DeleteUserData
    ${EndIf}

    DeleteUserData:
        RMDir /r "$APPDATA\tianshell"
        Goto CleanFiles

    KeepUserData:
        DetailPrint "用户配置数据已保留在: $APPDATA\tianshell"

    CleanFiles:
        ; 移除快捷方式
        Delete "$DESKTOP\${PRODUCT_NAME}.lnk"
        RMDir /r "$SMPROGRAMS\${PRODUCT_NAME}"

        ; 清理注册表项
        DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}"

        ; 清理安装目录文件
        Delete "$INSTDIR\${PRODUCT_EXE}"
        Delete "$INSTDIR\${PRODUCT_MCP_EXE}"
        Delete "$INSTDIR\WebView2Loader.dll"
        Delete "$INSTDIR\icon.ico"
        Delete "$INSTDIR\uninstall.exe"
        RMDir /r "$INSTDIR\ui"

        ; 若安装目录已空则移除目录本身
        RMDir "$INSTDIR"

        SetAutoClose true
SectionEnd
