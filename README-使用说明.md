# Foxmail Reader 使用说明

Foxmail Reader 是 Rust / GPUI 实现的 Windows x64 邮件客户端。
本文描述当前开发分支；旧发布包可能只有只读浏览能力，不能仅凭版本号推断功能。

当前源码已有邮件索引浏览/搜索、账号配置与 IMAP 增量同步、EML 导入、
写信/回复/转发/草稿、附件保存与图片预览，以及深浅/自定义主题。
**配置账号后，程序会连接服务器并在启动及定时任务中同步邮件，不再是纯只读工具。**

发送已统一到单账号模型：账号配置（首启引导页）可同时填写 IMAP 收信与
可选 SMTP 发信，发件身份在点击发送时从账号配置冻结快照中取得；SMTP 可选，
未配置时仍可收信，但写信窗口会提示无法发送。密码以 DPAPI 加密落盘，
日志与错误提示不含密码或完整地址列表。真实账号的发送/接收仍需按验收计划
另行人工验收，不要把自动测试通过当作真实账号可用承诺。

## 启动

双击 `xiou.exe`。程序按以下顺序查找索引：

1. `FOXMAIL_INDEX_PATH` 指定的 SQLite 文件。
2. `FOXMAIL_BACKUP_DIR` 目录下的 `mail-index.sqlite`。
3. 程序同目录 `data\mail-index.sqlite`。
4. `%LOCALAPPDATA%\foxmail-rs\data\mail-index.sqlite`。

普通使用时，将索引放到 `%LOCALAPPDATA%\foxmail-rs\data`。便携使用时，在
`xiou.exe` 同目录创建 `data` 文件夹并放入索引。环境变量的优先级高于
便携目录；切换模式前请检查是否保留了旧环境变量。

## 日志

- 普通模式：`%LOCALAPPDATA%\foxmail-rs\logs\foxmail-rs.log`
- 便携模式：程序同目录 `data\logs\foxmail-rs.log`

日志只记录时间、事件、索引来源和 SQLite/I/O 数字码，不记录邮件正文、凭据、
完整邮箱地址、完整索引路径或数据库原始错误。

## 常见问题

- 提示“未找到邮件索引”：按上述任一方式放置 `mail-index.sqlite`，然后重新启动。
- 提示索引损坏或格式无效：重新运行项目的 `import-eml` 工具生成索引。
- 提示日志写入失败：检查对应日志目录的写入权限和磁盘空间。
- 移动到其他电脑：复制 EXE 和 `data\mail-index.sqlite`，不要复制 `data\logs`；
  不要把凭据、原始邮件或日志发给他人。

## 校验下载

在 ZIP 所在目录运行：

```powershell
Get-FileHash -Algorithm SHA256 .\xiou-*-windows-x64.zip
```

结果应与同目录 `.sha256` 文件一致。ZIP 内的 `SHA256SUMS.txt` 用于校验 EXE。

## 常用快捷键

| 操作 | 快捷键 |
| --- | --- |
| 写信 / 回复 / 转发 | Ctrl+N / Ctrl+R / Ctrl+Shift+F |
| 搜索 | Ctrl+F |
| 立即同步 | Ctrl+Shift+S |
| 设置 | Ctrl+, |
| 写信窗口发送 | Ctrl+Enter；输入法组合态不得误发 |

## 当前边界

- GPUI Kit 迁移已有实现、离线自动回归及一批离屏截图审阅，但还未完成完整交付验收。
- 真实 Windows 输入法、托盘/焦点、多 DPI、代表性大图片/附件与完整 release 性能仍待验收。
- SMTP 发送走单账号配置（阶段二）；没有安装器和自动更新。不得把自动测试通过当成真实账号可用承诺。
- 新开发版与桌面快捷方式指向的旧 release 可能不同。升级前先核对 EXE 路径及生成时间。
- 当前进度与剩余验收项见 `docs/GPUI-MIGRATION.md`。
