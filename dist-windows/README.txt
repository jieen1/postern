Postern — 原生 Windows 包
==========================

最简用法：双击 posternd.exe
---------------------------
  双击 posternd.exe 即可。首次会自动在 %LOCALAPPDATA%\postern 下建库/保险箱/密钥/
  control-token，然后启动，监听 127.0.0.1:7878（控制面）/ 127.0.0.1:7879（数据面）。
  不需要 .bat、不需要设环境变量。

  (start-postern.bat 仍保留，只是显式版；现在不需要了。)

文件
----
  posternd.exe   守护进程（双击即用）
  postern.exe    命令行客户端（postern.exe daemon status 查健康）
  build-console.ps1  在 Windows 上构建桌面 GUI（见下）

数据/凭据位置
-------------
  %LOCALAPPDATA%\postern\
    policy.db / vault.postern / key / control.token

安全说明
--------
  Windows 版只用 control-token + 仅 127.0.0.1 本地回环（无内核 SO_PEERCRED uid 门，
  Windows 无此机制——你已同意的本机单用户模型）。

桌面 GUI（Postern Console）
---------------------------
  GUI（Tauri）的 Windows 安装包需在 Windows 上构建一次（要 WebView2 + 原生工具链）：
    - 需要 Rust+cargo、Node+pnpm、WebView2（Win10+ 自带）
    - PowerShell 在仓库根跑：  .\dist-windows\build-console.ps1
    - 产物在 web\src-tauri\target\release\bundle\（.msi / .exe）
  GUI 默认连 127.0.0.1:7878；启动前确保 posternd.exe 在跑。

注意：这是 Linux 交叉编译 + 移植产物，我无法在 Windows 实测；有报错发我即修。
