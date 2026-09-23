# macOS 支持现状调研

调研日期：2026-09-23

## 结论

BililiveRecorder 官方当前没有 macOS 桌面 GUI，也没有 DMG/PKG 发布物。官方支持 macOS 的是命令行版，且同时提供 Apple Silicon 与 Intel 二进制压缩包。

## 官方依据

1. 官方“录播姬的各个版本”页面明确说明：桌面版提供 Windows 图形界面；命令行版本身可在 Windows、Linux 和 macOS 上运行。
2. 官方“下载使用桌面版”页面的最新压缩包链接是 `BililiveRecorder-WPF-Portable.zip`，WPF 桌面实现只适用于 Windows。
3. 官方“命令行版”文档给出了 Linux/macOS 的启动方式，并要求执行 `./BililiveRecorder.Cli`。
4. BililiveRecorder `v2.20.0`（发布于 2026-09-19）的官方 Release 资产包含：
   - `BililiveRecorder-CLI-osx-arm64.zip`
   - `BililiveRecorder-CLI-osx-x64.zip`
   - Windows/Linux CLI 资产
   - `BililiveRecorder-WPF-Portable.zip`
5. 同一 Release 不包含 `.dmg`、`.pkg`、macOS `.app.zip` 或其他 macOS GUI 资产。
6. 上游 README 同样区分 Windows 桌面版与可运行于 Linux、macOS、Windows 的 CLI 二进制。

## 来源

- 官方桌面版文档：`https://rec.danmuji.org/install/desktop/`
- 官方版本说明：`https://rec.danmuji.org/install/versions/`
- 官方命令行版文档：`https://rec.danmuji.org/install/cli/`
- 官方 Release：`https://github.com/BililiveRecorder/BililiveRecorder/releases/tag/v2.20.0`
- 上游源码与 README：`https://github.com/BililiveRecorder/BililiveRecorder`
- 上游 WebUI：`https://github.com/BililiveRecorder/BililiveRecorder-WebUI`
- Tauri sidecar 官方文档：`https://v2.tauri.app/develop/sidecar/`

## 对实现的影响

- 录制能力来自官方 CLI，而不是在桌面壳中重新实现或模拟。
- CLI 已内置 Web API 与官方 WebUI，因此桌面端不复制、不修改 WebUI，只负责可靠分发、启动、健康检查、窗口承载和生命周期管理。
- 只绑定随机 `127.0.0.1` 端口，避免把无认证管理 API 暴露到网络。
- 官方文档要求使用 `Ctrl+C`/`SIGINT` 退出以保存配置并完整停止录制；macOS/Linux 实现遵循该要求，并设置强制终止超时兜底。
- 上游核心与 WebUI 均为 GPL-3.0，本仓库采用 GPL-3.0-only，保留来源和版本信息。
- 每日同步工作流追踪最新上游 Release；合并自动同步 PR 后，发布工作流生成签名桌面更新，已安装客户端通过 Tauri updater 获取新的 CLI 与其内嵌 WebUI。
