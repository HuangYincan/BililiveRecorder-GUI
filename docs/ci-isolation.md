# 隔离运行验证边界

当前 `Build desktop bundles` 在 GitHub-hosted `macos-26`、`ubuntu-22.04`、`windows-latest` runner 上执行 `cargo test`、CLI `--version`（Linux）和安装包构建；这些通过只证实代码可编译及合成子进程集成测试，不证实安装包退出、Windows Job Object 实际失败/杀树行为或录制落盘。Windows 上 `sidecar_lifecycle.rs` 有 `#![cfg(unix)]`，因此 17 项 Unix 生命周期集成用例**不会**在 Windows 执行。

GitHub-hosted runner 是每个 job 新建、完成后丢弃的执行环境，具备承载*本 job 启动的*应用/CLI 的隔离条件；但现有工作流**没有**在任何 runner 上执行 `artifact_lifecycle.rs` 的两个 `#[ignore]` 产物终止用例，也没有把打包可执行文件映射到 `BILILIVE_ARTIFACT_APP`。不因 runner 环境隔离就擅自给共享宿主、用户录制实例运行终止测试。

要补齐验证，还需先确认 CI 运行包的路径与签名/启动权限，Linux 配置显示服务或 `xvfb`，macOS Apple Event 针对从 bundle 中启动的进程是否能定位（当前测试仅按应用名 `osascript quit`），Windows 无控制台管道继承、真实 Job Object 失败注入及是否误伤同路径独立实例；每个平台都须对本次隔离运行采集应用实际版本与自身后端 PID、端口关闭、进程退出和真实录制输出的可读性/完整性，不能把旧日志关键词或端口关闭当作文件落盘。若缺少这些条件，则报告未验证，不得自动切换到共享宿主测试。
