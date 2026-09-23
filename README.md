# BililiveRecorder GUI

基于 Tauri 的跨平台 BililiveRecorder 桌面端。应用没有自建首页、启动器、导航或设置页：启动后在后台自动运行官方 CLI，唯一主窗口直接加载该 CLI 内嵌的官方 WebUI。Tauri 只负责后端分发、自动启动、生命周期管理与桌面更新。

## 当前状态

- macOS：已在 Apple Silicon macOS 26.6.2 上完成前端构建、Rust 检查、sidecar 启动/健康检查/优雅停止测试和 DMG 构建。
- Windows：已提供 x64 sidecar 映射和构建流水线，尚未在 Windows 实机验证。
- Linux：已提供 x64/arm64 sidecar 映射和构建流水线，尚未在 Linux 实机验证。

## 工作方式

1. `upstream.json` 记录唯一的上游版本来源。
2. `scripts/prepare-sidecar.mjs` 从 BililiveRecorder 官方 Release 下载与 Tauri target triple 对应的完整 CLI 压缩包；官方 WebUI 已嵌入该 CLI，因此不会发生手工搬运 UI。
3. Tauri 将 CLI 的完整 .NET 自包含运行目录作为资源随桌面安装包分发。
   Linux 打包仅移除 CoreCLR 的可选 LTTng tracing provider；该库在 Ubuntu 22.04 缺少其旧 ABI 依赖，且 .NET 运行时明确允许 provider 不存在。正式构建会先执行 CLI 版本检查，再生成 deb、rpm、AppImage 与签名更新包。
4. 应用启动时自动创建系统视频目录下的 `BililiveRecorder` 工作目录，为后端分配随机本机端口并在后台执行：

   ```text
   BililiveRecorder.Cli run --bind http://127.0.0.1:<port> <工作目录>
   ```

5. `/api/version` 健康检查成功后才创建唯一主窗口，窗口 URL 直接指向 CLI 内置 WebUI；不存在本地前端页面或二次入口。
6. 关闭唯一主窗口与显式退出应用的结果相同：应用只停止自身持有的 sidecar 句柄，不按进程名扫描或终止用户独立运行的录播姬。macOS/Linux 先发送 `SIGINT`，超时后才强制终止；句柄析构还提供最终回收兜底。Windows 当前使用强制终止兜底，详见“已知限制”。
7. 再次启动会创建新的应用进程、sidecar 进程和随机本机端口，不复用上一次运行的 PID 或监听端口。

## 上游同步与桌面更新

1. `.github/workflows/sync-upstream.yml` 每天查询 BililiveRecorder 最新官方 Release。
2. 发现新版本后，自动更新 `upstream.json`、npm/Tauri/Cargo 版本，校验各平台官方 CLI 资产，并创建启用自动合并的同步 PR。
3. 自动合并到 `main` 后，`.github/workflows/release.yml` 在 macOS ARM64、macOS x64、Windows x64、Linux x64 上重新下载对应官方 CLI 并构建桌面安装包。
4. 发布流程使用 Tauri updater 私钥签名更新包，并生成 GitHub Release 的 `latest.json`。
5. 已安装应用启动后通过系统原生对话框提示更新；验证签名后原位更新并重启，不引入自建更新页面。

因此 WebUI 的版本同步依赖上游官方 CLI 产物本身，不需要本仓库复制或追踪 WebUI 源文件。已安装用户通过签名桌面更新获得新的 CLI 与其内嵌 WebUI。

## 开发

需要 Node.js 24+、Rust 1.84+ 以及对应平台的 Tauri 系统依赖。

```bash
npm install
npm run sidecar:prepare
npm run desktop:dev
```

构建当前平台安装包：

```bash
npm run desktop:build
```

当前使用的上游版本记录在 `upstream.json`。可在临时构建时覆盖下载版本：

```bash
BILILIVE_RECORDER_VERSION=v2.20.0 npm run sidecar:prepare
```

## 调研结论

截至 2026-09-23，上游“桌面版”仍是 Windows WPF 版本，官方桌面文档只提供 `BililiveRecorder-WPF-Portable.zip`；官方版本说明明确称桌面版提供 Windows 图形界面，而命令行版支持 Windows、Linux 和 macOS。`v2.20.0` Release 提供 `BililiveRecorder-CLI-osx-arm64.zip` 与 `BililiveRecorder-CLI-osx-x64.zip`，但没有 `.dmg`、`.pkg` 或 macOS GUI 产物。因此：**macOS 有官方 CLI，没有官方 macOS 桌面 GUI。**

完整依据与资产清单见 `docs/research.md`。

## 已知限制

- Windows sidecar 由 Tauri 以无控制台方式启动，现阶段无法可靠发送 `Ctrl+C`；停止时会等待后端自行退出，随后强制终止。需要在 Windows 实机上继续验证配置持久化行为。
- Linux 与 Windows 安装包尚未实机验证；GitHub Actions 工作流用于持续补齐构建结果。
- sidecar 下载依赖 GitHub Release；正式发布应同时保存资产校验值并对桌面安装包签名/公证。
- Tauri updater 包已有独立签名链；macOS Developer ID 签名与 Apple 公证仍需仓库所有者提供 Apple 凭据。
- 应用不向局域网开放后端，也没有提供修改监听地址的入口。

## 许可证

本项目按 GPL-3.0-only 分发。随安装包分发的 BililiveRecorder CLI 以及其内置 WebUI 同样由上游按 GPL-3.0 发布。具体来源、版本与再分发说明见 `THIRD_PARTY_NOTICES.md`。
