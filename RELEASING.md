# 发布 Foxmail Reader

## 固定输入

- Windows x64
- Rust `1.97.1` stable `x86_64-pc-windows-msvc`
- `Cargo.lock`，构建必须使用 `--locked`
- GPUI / Platform `0.3.4`，GPUI Kit / Component / Base `0.6.1`，以 `Cargo.lock` 为准
- MSVC Tools `14.44.35207`、`link.exe 14.44.35228.0`
- Windows SDK `10.0.26100.0`
- PowerShell `7.6.6` 和 .NET `ZipArchive`

不要从未提交差异构建正式包。先运行质量门禁，再执行：

```powershell
.\scripts\package-release.ps1
```

脚本会校验 Rust，并用 Visual Studio 官方开发环境脚本选择指定的 MSVC linker 和
Windows SDK；随后临时固定仓库内 `target` 目录和 x64 target、忽略外部 Cargo
编译器、target、Release profile 与原生 C/C++ 构建配置，离线锁定依赖，清除
构建机路径，使用静态 CRT 和 `/Brepro` 构建 `xiou.exe`，并从 Cargo JSON
输出读取实际产物路径。退出时恢复原进程环境。
随后在 `dist` 生成版本化 Windows x64 ZIP 和对应 `.sha256` 文件。ZIP 只包含：

- `xiou.exe`
- `README-使用说明.md`
- `SHA256SUMS.txt`

连续两次运行在相同工具链和输入下必须产生相同 ZIP SHA-256。发布前还必须扫描
EXE 和 ZIP，确认不含构建用户名路径、凭据、真实邮箱、邮件、索引或日志。

## 当前包状态：候选版（2026-09-21 阶段二完成后解除"测试候选"限制）

2026-09-22 起的 `xiou-0.1.0-windows-x64.zip` 已完成阶段二账号与 SMTP 改造：
源码中不再存在任何编译期固定的邮箱地址、SMTP/IMAP 主机或构建机路径
（原 `zhangwei@szshuori.com`、`smtp/imap.yunyou.top` 常量已随
`src/imap_config.rs` 一并删除）。复扫确认 EXE 内除运行时数据目录名
`%LOCALAPPDATA%\foxmail-rs` 外不含任何身份信息。双次独立打包 SHA-256 一致
（记录见 `dist/_zip-run-A.txt`、`dist/_zip-run-B.txt`）。

历史记录：2026-09-21 之前的包为测试候选版，当时写信/IMAP 链路编入了
开发期固定常量，不得交付；这些常量现已全部移除。


## 迁移期候选版与正式包的区别

2026-09-14 的 E8 候选版构建、离屏截图和隔离内存 smoke 不是正式发布许可。
正式发布前还要完成 `docs/GPUI-NATIVE-ACCEPTANCE.md` 与迁移清单的剩余门禁，
处理旧控件/spike、确认 SMTP PoC 配置边界，并在授权后提交源码。
本轮保留既有 spike 与 dead_code 警告，不以删除它们代替功能验收。

如只做性能/兼容性验证，应显式使用独立 target 目录，例如：

```powershell
cargo build --locked --offline --release --bin xiou --target-dir target/release-review
```

该命令不等于 `package-release.ps1` 的固定工具链、静态 CRT、可复现 ZIP 验证；
不得以它的成功跳过发布脚本或声称 ZIP 可复现。不要直接覆盖现用 release EXE，
也不要从真实用户配置启动自动测试。所有账号、索引、草稿、日志和临时目录必须隔离。
