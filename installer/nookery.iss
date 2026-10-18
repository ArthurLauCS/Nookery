; Nookery 安装包（Inno Setup 6）。用 scripts\installer.ps1 构建：它先编译、给右键菜单的稀疏包签名，再传入版本号和证书指纹。
; 按当前用户安装到 %LOCALAPPDATA%\Programs\Nookery。只有一步要管理员权限：把签名证书装进本机“受信任人”，
; Win11 才肯注册稀疏包，桌面右键菜单第一级才有“新建磁贴”。证书已经装过就不再弹 UAC。

#ifndef Version
  #define Version "0.0.0"
#endif

[Setup]
AppId=Nookery
AppName=Nookery
AppVersion={#Version}
AppVerName=Nookery {#Version}
DefaultDirName={autopf}\Nookery
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\target\installer
OutputBaseFilename=Nookery-Setup-{#Version}
SetupIconFile=..\assets\app.ico
UninstallDisplayIcon={app}\nookery.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; 每次都让用户选语言，选中的语言也是程序的界面语言
UsePreviousLanguage=no
; 升级时结束正在运行的实例和加载着右键菜单 DLL 的 dllhost：主窗口是隐藏窗口，不强制的话关不掉
CloseApplications=force
RestartApplications=no

[Languages]
Name: "zh"; MessagesFile: "ChineseSimplified.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
zh.AutoStart=开机自动启动
en.AutoStart=Start with Windows
zh.ContextMenu=正在添加桌面右键菜单…
en.ContextMenu=Adding the desktop context menu...

[Tasks]
Name: "autostart"; Description: "{cm:AutoStart}"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked

[Files]
Source: "..\target\release\nookery.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\nookery_menu.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\installer\nookery.msix"; DestDir: "{tmp}"
Source: "..\target\installer\nookery.cer"; DestDir: "{tmp}"

[Icons]
Name: "{autoprograms}\Nookery"; Filename: "{app}\nookery.exe"
Name: "{autodesktop}\Nookery"; Filename: "{app}\nookery.exe"; Tasks: desktopicon

[Registry]
; 和托盘菜单“开机自启”写的是同一个值，两边开关互认
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Nookery"; ValueData: """{app}\nookery.exe"""; Tasks: autostart
; 界面语言（zh / en），程序和右键菜单扩展都读这里，托盘菜单切换时改写
Root: HKCU; Subkey: "Software\Nookery"; ValueType: string; ValueName: "Language"; ValueData: "{language}"; Flags: uninsdeletekey

[Run]
Filename: "{sys}\certutil.exe"; Parameters: "-f -addstore TrustedPeople ""{tmp}\nookery.cer"""; Verb: "runas"; Flags: shellexec runhidden waituntilterminated; Check: CertMissing; StatusMsg: "{cm:ContextMenu}"
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -Command ""Add-AppxPackage -Path '{tmp}\nookery.msix' -ExternalLocation '{app}' -ForceUpdateFromAnyVersion"""; Flags: runhidden waituntilterminated; StatusMsg: "{cm:ContextMenu}"
Filename: "{app}\nookery.exe"; Description: "{cm:LaunchProgram,Nookery}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; 先结束程序，免得刚放回桌面的文件又被收进磁贴；右键菜单 DLL 在 dllhost 里，也要结束才删得掉
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM nookery.exe"; Flags: runhidden; RunOnceId: "Kill"
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM dllhost.exe /FI ""MODULES eq nookery_menu.dll"""; Flags: runhidden; RunOnceId: "KillMenu"
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -Command ""Get-AppxPackage Nookery | Remove-AppxPackage"""; Flags: runhidden; RunOnceId: "Package"
; 文件全部放回桌面，删开机自启和老式右键菜单
Filename: "{app}\nookery.exe"; Parameters: "--uninstall"; RunOnceId: "Restore"
Filename: "{sys}\certutil.exe"; Parameters: "-delstore TrustedPeople {#CertThumb}"; Verb: "runas"; Flags: shellexec runhidden waituntilterminated; RunOnceId: "Cert"

[UninstallDelete]
; 配置一起删，重装后第一次运行会重新确认要不要整理桌面
Type: filesandordirs; Name: "{userappdata}\Nookery"

[Code]
function CertMissing: Boolean;
begin
  Result := not RegKeyExists(HKLM, 'SOFTWARE\Microsoft\SystemCertificates\TrustedPeople\Certificates\{#CertThumb}');
end;
