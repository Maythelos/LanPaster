LanPaster __VERSION__ - Windows x64
================================

文件说明
--------
- lanpaster.exe          正常使用：双击启动托盘，不弹控制台窗口
- lanpaster-console.exe  命令行 / 调试用：会打开控制台，可执行 init、version 等命令

使用
----
1. 双击 lanpaster.exe 启动，托盘出现 LP 图标。
2. 左键托盘图标打开设置，右键打开菜单。
3. 首次运行会自动创建配置：
   %APPDATA%\LanPaster\config.toml
4. 接收到的文件缓存：
   %LOCALAPPDATA%\LanPaster\cache\incoming\
5. 局域网同步如被 Windows 防火墙拦截，请在弹窗中允许“专用网络”。

注意
----
- 程序未做代码签名，Windows SmartScreen 可能提示；选择“更多信息”后“仍要运行”。
- 本包是在 macOS 上使用 LLVM-MinGW 交叉编译的 Windows x64 版本（x86_64），尚未在 Windows 实机验证。
- 支持 Windows 10 / 11 x64；不支持 32 位 Windows。
- 两个 exe 共用同一份配置，不能同时运行；正常使用只需要 lanpaster.exe。
