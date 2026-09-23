# 性能基线与预算（N7）

## 方法

- 语料：`C:/Users/lzzwf/Desktop/foxmail-e9-20260920/corpus`（4 个 `.invalid`
  账号、5 邮箱、10,013 封，含 100 封 ~200 KB 长正文与 20 封 ~1 MB 内联 PNG）。
- 工具：`measure.ps1`（隔离 LOCALAPPDATA/APPDATA/TEMP + 索引/备份环境变量，
  100 ms 采样）；搜索计时用 `import-eml --search`（真实 SQLite/FTS5 路径，
  进程级壁钟计时，6 次）。
- 被测 EXE：`xiou-0.1.0-windows-x64.zip`（多账号版，EXE SHA-256
  `acfc57f0…2ea4a`，ZIP `a1945985…701768`）。

## 启动（AUTO_EXIT_MS=6000，3 次，均 exit 0、无挂起）

| 指标 | 数值 |
| --- | --- |
| 峰值工作集 | 73.4 – 85.6 MiB |
| 峰值 private bytes | 75.0 – 82.3 MiB |
| 壁钟 | 6.4 – 6.5 s（≈6 s 为 auto-exit 驻留） |

与 2026-09-20 E9 候选基线（85.38–89.58 MiB WS）持平，多账号改造无启动内存回归。

## 10 分钟驻留（600 s × 3，5 s 采样粒度）

- 全程无挂起（`hung_detected=false` × 3）；结束值 61.0–82.8 MiB WS，
  与峰值同量级，无单调增长趋势 → 无泄漏迹象。
- run 1 出现一次性 private 峰值 155.5 MiB（启动期，索引/同步初始化阶段），
  随后回落到 82.1 MiB 平台；run 2/3 峰值 77.7 / 71.0 MiB。按启动期瞬时
  峰值记录，不按泄漏处理。
- 对照 2026-09-20（WS 85.0→76.4 平台期）：量级一致。

## 搜索（10,013 封，FTS5）

- 固定成本（进程 + 开库 + 计数 + 完整性检查）：817–830 ms/次
  （CLI 含 `integrity_check`；GUI 打索引走 read_only 路径，无此开销，
  但 GUI 端数字未单独测——列为未测项）。
- 查询边际成本（预热缓存）：
  - `E9`（≈1 万封命中，排名取前 50）：835–851 ms 总壁钟 → 边际 **10–25 ms**；
  - `基准`（CJK）：10–14 ms 边际；多词无命中：+70–80 ms；纯无命中：+3–26 ms。
- 冷缓存单次离群 1.58 s（首次跑该查询时）；后续均 <100 ms 边际。

## 预算（10k 封语料；超出即视为回归，须先优化再发布）

| 项 | 预算 | 当前实测 | 余量 |
| --- | --- | --- | --- |
| 启动峰值工作集 | ≤ 120 MiB | 85.6 MiB | ~34% |
| 启动峰值 private | ≤ 120 MiB | 82.3 MiB | ~32% |
| 驻留 10 分钟 | 无单调增长，结束 ≤ 峰值 | 通过（3/3） | — |
| FTS 查询边际延迟（预热） | P95 ≤ 200 ms | 10–25 ms | ~8× |
| 启动期瞬时 private 峰 | ≤ 250 MiB | 155.5 MiB（1/3） | ~38% |

预算由 2026-09-23 实测上浮 ~40% 得出（吸收机器差异），非拍脑袋数字；
后续每批发布在 10k 语料上重跑 startup/dwell 即可对照。

## 未覆盖（明确不做，避免"全绿"错觉）

- GUI 内滚动/翻页/长正文渲染延迟（OS 注入测量不可靠，需要应用内埋点）。
- GUI 端开索引延迟（read_only 路径未单独计时）。
- 100k+ 语料（当前语料只有 10k；预算外推不成立，届时需重测）。
- 多账号同时 10k 语料（本语料 4 账号但只占 10k 总量）。

## 复跑命令

```powershell
# 启动（3 次）
pwsh -NoProfile -File C:\Users\lzzwf\Desktop\foxmail-e9-20260920\measure.ps1 `
  -Exe <xiou.exe> -Root C:\Users\lzzwf\Desktop\foxmail-e9-20260920 -Mode startup -Runs 3
# 驻留（10 分钟）
pwsh -NoProfile -File ... measure.ps1 -Exe <xiou.exe> -Root ... -Mode dwell -DwellSeconds 600
# 搜索计时
target\release\import-eml.exe --database <10k索引> --search "E9" --limit 50
```
