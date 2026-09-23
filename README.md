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
6. 后端由**守护进程**持有：应用自己从不接触后端的进程号，而是启动一个守护进程并把要执行的命令交给它，由守护进程把后端创建为**它自己的子进程**，也只有它会向后端发信号或回收它。
   - 未被回收的子进程在回收前会一直保留它的 PID，因此不存在“PID 被复用后杀错进程”的可能，也不存在“先核对身份、再发信号”之间的竞态窗口。
   - 代码中没有任何 `/proc`、`proc_pidpath`、进程名或进程组查询；用户自己启动的录播姬——包括从同一路径启动的第二份——在结构上就不可达。
   - 停止时先发 `SIGINT` 让录播姬落盘，超过 8 秒才强制终止；Windows 无法发送 `Ctrl+C`，直接进入强制终止，详见“已知限制”。
7. 关闭主窗口、显式退出应用、收到 `SIGTERM`/`SIGHUP`/`SIGINT`，都走同一套关闭流程；应用被 `SIGKILL` 或崩溃时，守护进程通过管道关闭得知应用已消失并自行完成后端回收。
8. 再次启动会创建新的应用进程、守护进程、后端进程和随机本机端口，不复用上一次运行的 PID 或监听端口。

## 上游同步与桌面更新

1. `.github/workflows/sync-upstream.yml` 每天查询 BililiveRecorder 最新官方 Release。
2. 发现新版本后，自动更新 `upstream.json`、npm/Tauri/Cargo 版本，校验各平台官方 CLI 资产，并创建启用自动合并的同步 PR。
3. 自动合并到 `main` 后，`.github/workflows/release.yml` 在 macOS ARM64、macOS x64、Windows x64、Linux x64 上重新下载对应官方 CLI 并构建桌面安装包。
4. 发布流程使用 Tauri updater 私钥签名更新包；各平台构建完成后统一聚合并校验 GitHub Release 的 `latest.json`，避免矩阵任务互相覆盖更新清单。
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

- **只有 macOS 经过实测，不宣称跨平台保证已成立。** 守护进程不使用任何平台特有接口（一个 `Child` 加两条管道），因此在 Windows/Linux 上同样构建与分发，所有权论证也照样成立（Windows 在进程句柄打开期间同样不会复用 PID，`Child` 会持有该句柄直到 `wait`），但这是推理而非实测。Windows 上以下行为从未被观测：应用死亡是否可靠关闭守护进程阻塞的那条管道写端（整条崩溃/`SIGKILL` 路径都依赖它）、是否有其他进程持有一份写端副本导致守护进程永不唤醒、无控制台启动时守护进程自身的启动与退出行为。
- Windows 后端无法被礼貌停止（无控制台子进程收不到 `Ctrl+C`），停止时直接强制终止，不给 CLI 落盘机会。守护进程会为后端建立 **Job Object**（`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`），因此守护进程被杀时内核会连同后端一起结束；Linux 同理由 `PR_SET_PDEATHSIG` 覆盖。**这两段平台代码只做过类型检查，从未运行过**（见下）。
- 守护进程被杀或卡死时的收敛（按平台分层）。前提是**本程序只对自己创建的、尚未回收的 `Child` 发信号**——因为内核在此期间保留其 PID。**它绝不向任何别处读来的 PID 发信号**：不读内核子进程列表，不读日志，不读上报。
  - 早先的实现违反了这条：为收敛卡死的守护进程，它从内核读出守护进程的子进程再逐个发信号，理由是「读取与发信号在同一轮循环内」。**该论证不成立**——那些子进程不由本进程持有，其中一个可以在读取与发信号之间退出并被守护进程回收，内核随后可能把它的 PID 交给无关进程，也就是本次事故那一类误杀。读一个存活父进程的子进程列表并不等于拥有它们，反复重读也不会让它变成原子操作。该路径已**整体删除**，没有替代品。
  - **守护进程卡死**：Linux / Windows 上内核已经把后端与守护进程绑定，因此结束守护进程（应用自己持有的 `Child`）就能一并回收后端，既充分又精确。**macOS 上不做任何事**——见下。
  - **守护进程被杀**：Linux 由 `PR_SET_PDEATHSIG`、Windows 由 Job Object 在内核层解决。
  - **Windows 的约束在后端存在之前就已建立**：作业对象在守护进程启动时创建，并把**守护进程自身**加入（Windows 作业会被子进程继承，后端及其派生的进程在创建时自动入组）。因此不存在后端脱离约束运行的窗口，也不存在「先运行、后加入作业」的竞态。建立失败时守护进程以非零码退出且**不启动任何东西**——此刻尚无进程被创建，所以没有需要收敛的对象，失败分支上的句柄也会关闭。
- **macOS 卡死场景的技术取舍（需决策，不是遗漏）**：macOS 既无 `PR_SET_PDEATHSIG` 也无作业对象。守护进程被强杀时后端会被 `launchd` 收养，应用既无法证明归属也不能安全发信号，只能点名其 PID 供人处理。守护进程**卡死**时应用只有两个选择，而安全的只有一个：
  - **结束守护进程**：在 macOS 上回收不了任何东西，反而**保证**后端被孤儿化，严格劣于什么都不做；
  - **不动它**（当前实现）：守护进程仍然持有后端，一旦恢复就会把它关掉；而且应用退出会关闭 keepalive 管道，因此守护进程此后任何时刻恢复都会自行关停后端。

  代价是：macOS 上守护进程卡死时，后端可能继续运行，直到守护进程恢复或有人手工停止——应用会明确告知，而不是假装已经收敛。它**不会**去拿后端的 PID 强行回收：一次可能打到无关进程的强制回收，不值得用来换取「看起来关干净了」。要真正堵上需要内核级回收器（launchd agent 或系统扩展）来收养孤儿，其签名与公证成本与这个缺陷不相称，因此留作维护者的决策而非默认假设。
- **平台代码的验证程度**：`src-tauri/src/platform.rs` 已对 macOS / Linux / Windows 三个目标做过 `cargo check`（借助独立的最小 crate 绕开 tauri 的 C 依赖），但**从未在 Linux 或 Windows 上运行过**。`src-tauri/tests/artifact_lifecycle.rs` 是跨平台的产物级终止测试，同样只在本机编译过，两个用例默认 `#[ignore]`，需要可丢弃机器才会执行。
- Linux 与 Windows 安装包尚未实机验证；GitHub Actions 工作流用于持续补齐构建结果。
- sidecar 下载依赖 GitHub Release；正式发布应同时保存资产校验值并对桌面安装包签名/公证。
- Tauri updater 包已有独立签名链；macOS Developer ID 签名与 Apple 公证仍需仓库所有者提供 Apple 凭据。
- 应用不向局域网开放后端，也没有提供修改监听地址的入口。

## 许可证

本项目按 GPL-3.0-only 分发。随安装包分发的 BililiveRecorder CLI 以及其内置 WebUI 同样由上游按 GPL-3.0 发布。具体来源、版本与再分发说明见 `THIRD_PARTY_NOTICES.md`。
