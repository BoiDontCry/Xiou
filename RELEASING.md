# 发布 Foxmail Reader

## 固定输入

- Windows x64
- Rust `1.97.1` stable `x86_64-pc-windows-msvc`
- `Cargo.lock`，构建必须使用 `--locked`
- GPUI/Zed 提交 `5415508d610542656c3ec9776cd05230c73ecda7`
- MSVC Tools `14.44.35207`、`link.exe 14.44.35228.0`
- Windows SDK `10.0.26100.0`
- PowerShell `7.6.3` 和 .NET `ZipArchive`

不要从未提交差异构建正式包。先运行质量门禁，再执行：

```powershell
.\scripts\package-release.ps1
```

脚本会校验 Rust，并用 Visual Studio 官方开发环境脚本选择指定的 MSVC linker 和
Windows SDK；随后临时固定仓库内 `target` 目录和 x64 target、忽略外部 Cargo
编译器、target、Release profile 与原生 C/C++ 构建配置，离线锁定依赖，清除
构建机路径，使用静态 CRT 和 `/Brepro` 构建 `foxmail-rs.exe`，并从 Cargo JSON
输出读取实际产物路径。退出时恢复原进程环境。
随后在 `dist` 生成版本化 Windows x64 ZIP 和对应 `.sha256` 文件。ZIP 只包含：

- `foxmail-rs.exe`
- `README-使用说明.md`
- `SHA256SUMS.txt`

连续两次运行在相同工具链和输入下必须产生相同 ZIP SHA-256。发布前还必须扫描
EXE 和 ZIP，确认不含构建用户名路径、凭据、真实邮箱、邮件、索引或日志。
