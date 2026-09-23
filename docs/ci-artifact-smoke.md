# CI 产物验证（独立于 PR #3 生命周期实现）

`Build desktop bundles` 在 GitHub-hosted macOS ARM64、Ubuntu x64、Windows x64 各自构建完成后运行 `npm run bundle:smoke`。这个检查不会启动或终止桌面应用，不使用用户工作目录；只在该 job 的临时目录解包自身生成的产物：macOS 从 `.app` 资源目录、Linux 从 `.deb`、Windows 通过 `msiexec /a` 管理映像从 `.msi` 定位所打包的 CLI，并执行 **该包内** CLI `--version`，比仅执行先前下载到 sidecar 暂存目录的 CLI 更接近真实交付。缺包、缺对应 target 的资源、运行失败或版本与 `upstream.json` 不同均令 job 失败。测试不下载任何已发布资产、不改变已公开 tag。

执行范围严格限定：这可验证三类包确实内置可启动的正确版本 CLI，**不是**安装包的 GUI 首屏/安装器运行测试，不代表 Windows Job Object 杀树、macOS AppleEvent、后端端口、录制文件落盘或 updater 下载验签。Linux GUI 会话的 AppImage/DEB/RPM 安装与更新、Windows 无控制台退出、macOS 真实产品终止测试仍缺独立环境跑通的用例与对应日志。PR #3 的 `artifact_lifecycle.rs` 尚未合并，本 PR 没有把不存在或 `#[ignore]` 的终止测试标为通过；与实现负责人约定的接口为：隔离 runner 运行时指定 `BILILIVE_ARTIFACT_APP` 指向该 runner 构建的打包应用、`BILILIVE_ARTIFACT_OK=1` 并将工作目录/日志指向独立临时位置，只在可丢弃 runner 上执行；是否启用还需解决 macOS AppleEvent、Linux 显示服务及 Windows GUI 会话与真实失败注入。所有验证由 runner 的 CI 结果独立报告，不转到共享主机。
