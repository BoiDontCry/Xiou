# Foxmail Reader 使用说明

Foxmail Reader 是 Windows x64 只读邮件查看器，用于浏览和搜索已经生成的
`mail-index.sqlite`。它不会连接服务器、发送邮件或修改索引。

## 启动

双击 `foxmail-rs.exe`。程序按以下顺序查找索引：

1. `FOXMAIL_INDEX_PATH` 指定的 SQLite 文件。
2. `FOXMAIL_BACKUP_DIR` 目录下的 `mail-index.sqlite`。
3. 程序同目录 `data\mail-index.sqlite`。
4. `%LOCALAPPDATA%\foxmail-rs\data\mail-index.sqlite`。

普通使用时，将索引放到 `%LOCALAPPDATA%\foxmail-rs\data`。便携使用时，在
`foxmail-rs.exe` 同目录创建 `data` 文件夹并放入索引。环境变量的优先级高于
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
Get-FileHash -Algorithm SHA256 .\foxmail-rs-*-windows-x64.zip
```

结果应与同目录 `.sha256` 文件一致。ZIP 内的 `SHA256SUMS.txt` 用于校验 EXE。

## 当前边界

当前版本是只读 MVP，不包含安装器、自动更新、账号配置、后台同步、写信、附件
保存或预览。
