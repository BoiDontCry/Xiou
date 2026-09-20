# GPUI Component 生态迁移与验收清单

> 当前进度以迁移矩阵及文末 2026-09-14 记录为准。历史章节保留当时结论，不代表当前仍停在 E5。

## 目标与权威来源

目标是把 Foxmail Reader 的现有完整界面和交互接入官方 GPUI Component 生态，
不是仅完成可编译的依赖替换，也不是只把 Button/Input 名字换掉。

用户提供的入口：<https://longbridge.github.io/gpui-component/zh-CN/>

2026-09-10 实际读取该入口，其标题为 GPUI Kit，官方快速开始推荐 `gpui-kit = "0.6"`：

- <https://gpui-kit.com/zh-CN/docs/getting-started>
- <https://gpui-kit.com/zh-CN/component>
- <https://gpui-kit.com/component/root>
- <https://gpui-kit.com/zh-CN/docs/assets>

本项目锁定 Kit / Component / Base 0.6.1，GPUI / Platform 0.3.4。
网页部分示例存在旧签名，具体 API 以对应锁定版本的官方 crate 源码为准。
继续通过 `gpui_kit::component` 使用成品组件，不为名称变化重复引入第二套 GPUI。

## 不变约束

- 保留邮箱加载、搜索、同步、草稿、回复/转发、附件和系统托盘的业务语义。
- 保留单栈、18 个 Foxmail 主题 token、搜索导航、发送快捷键和 IME 防误发约束。
- 各窗口必须具有官方 Root、资源源、主题和实际使用的浮层挂载点。
- 通用交互使用成品组件；邮件领域内容不能因为组件替换丢失 HTML 清洗、CID 图片、分页和性能能力。
- 不为“组件全用上”向邮件产品添加无需求的图表或 JavaScript 插件功能。
  对已有通用控件不得用“业务自绘”作为继续保留旧交互实现的借口。
- 所有自动测试隔离账号、索引、草稿和配置，不访问真实邮箱，不发送邮件。
- 不覆盖已有改动，不擅自暂存/提交。最终依赖锁定、CHANGELOG、提交属于原 T5 收尾项。

## 全量迁移矩阵

| ID | 产品表面 / 能力 | 目标组件 / 系统 | 当前状态与未完成项 | 验收证据 |
| --- | --- | --- | --- | --- |
| E0 | 依赖与初始化 | Kit façade、单一 GPUI、`init` | 单栈及官方生态方向已核对 | Cargo.lock、反向依赖树、官方入口 |
| E1 | 主窗口、Onboarding、写信窗口 | Root、Assets、Dialog/Sheet/Notification 层 | 两个窗口及 Onboarding/同步分支已接入并通过隐藏窗口运行检查；托盘及真实 OS 焦点仍待验收 | 原生根类型、资源加载、浮层实际渲染、Tab 流转与托盘回归 |
| E2 | 按钮、输入、Tooltip、布尔开关、分段选择 | Button、Input、Textarea、Tooltip、Switch、TabBar | 代码已接入 Button/Input/Textarea/Tooltip、3 个 Switch 和 4 组 TabBar；设置渲染及模拟 HEX Tab 路径通过，指针/全键盘/视觉验收仍待完成 | 真实状态/事件测试、键盘与视觉检查 |
| E3 | 自定义配色 | Select、Slider、Form/Field | 5 个可编辑基础 token 已用官方 Select/Slider/Form 接入，继续派生 18 个 Foxmail 主题 token；HEX 错误禁存、HSV 灰/黑色轨迹保留、保存/取消已补回归；视觉和真实 OS 验收仍待 | 默认/自定义深浅主题、全部基础 token、HEX↔HSV、指针/合成键盘、失败保留草稿 |
| E4 | 设置、附件预览、搜索浮层 | Root Dialog、Popover、ListItem | 设置/预览已接入实际业务 Dialog，搜索已接入 Popover/ListItem；关闭、焦点、确认键、空格及异步取消已补回归；真实 OS IME 与完整视觉验收仍待 | 业务状态、合成 Esc/外部点击/Tab/方向键/Enter、marked-text 防误选、有效视口截图 |
| E5 | 同步、保存、发送状态 | Alert、Progress、Notification | 业务接入与离线自动回归已完成；原生 OS／视觉联验仍在完整门禁内 | 代次隔离、下载／入库分阶段、保存与发送快照、通知去重及关闭／恢复均有回归 |
| E6 | 侧栏、邮件列表、搜索结果、滚动与分隔 | Tree、VirtualList/List、Scrollable、Resizable | 实现及离线回归已落地；9 月 14 日修复展开侧栏行内容重叠，并补多账号长标签/计数的真实布局断言；release 滚动性能验收仍待 | 10k 邮箱、跨页键盘导航、缓存上限、拖拽边界、四主题/窄窗口截图 |
| E7 | 工具栏图标、头像、计数、附件、正文 | Icon、Avatar、Badge、Attachment、TextView | 实现及离线回归已落地；补齐默认包之外的 12 个图标资源，23 个实际使用图标已渲染审阅；代表性大图/大附件与完整性能验收仍待 | MIME/CID/HTML 安全、纯文本、12k 行正文、附件事件回归及四主题截图 |
| E8 | 统一主题与跨窗口行为 | Theme、Component resolved tokens、Base、Root | 修复只更新颜色字段而未更新实际渲染 tokens 的问题；按钮/弹层/列表/进度条/滑块同步，已打开写信窗口有真实按钮像素断言；83 张现有离屏截图已审阅；OS/DPI 验收仍待 | 24 个布局检查 + 32 个截图/跨窗场景；独立审阅清单保留每张 PNG 的 SHA-256 |
| E9 | 完整交付 | 原 T4 / T5 | 已构建独立 release 候选版并完成隔离启动/内存对照，更新进度与使用/发布文档；OS 验收、完整预算复测、spike/旧代码清理、提交及正式打包未完成 | 本文最新证据与 GPUI-NATIVE-ACCEPTANCE.md |

## 原 T0–T5 与新目标的关系

T0 探针、T1 单栈/主题、T2 局部控件、T3 写信编辑器是必要基础，
不再把它们的局部绿灯当成 E0–E9 已完成。T4 必须覆盖迁移后的完整产品表面，
T5 在功能/交互完成后再做清理、体积/内存复测、发行记录及提交。

## 必须保留的自动门禁

```powershell
cargo check --locked --offline --bin xiou
cargo test --locked --offline --lib --bin xiou
cargo test --locked --offline --features native-ui-tests --test gpui-migration
cargo build --locked --offline --bin xiou
```

原生测试由显式 feature 开启，使用自己的 main 线程、隐藏窗口和独立临时目录。
`FOXMAIL_AUTO_EXIT_MS=3000` 的主程序冒烟必须单独运行，不能以测试窗口启动替代产品启动。

新增测试每次必须注明覆盖的是业务状态、合成 GPUI 事件、真实 OS 输入或截图，
不得将其中一类结果冒充另一类。未访问实际业务路径的“层已存在”也不能替代业务迁移。

## 最终验收门禁（全部有证据后才可完成整体目标）

- [ ] E1–E8 中的现有产品表面已完成迁移或有明确、经过验证的领域实现保留理由。
- [ ] Onboarding：4 字段编辑/遮罩/校验、Tab 顺序、连接状态、失败重试。
- [ ] 搜索：输入/清除/收起、上下选择/Enter、失焦/Esc、无结果/失败/等待态。
- [ ] 设置：所有开关/分段/token/HEX/HSV、保存/取消/外部点击、草稿不因失焦丢失。
- [ ] 写信：新建/回复/转发/草稿、to/cc/subject/body 焦点、普通换行与发送快捷键。
- [ ] 原生 IME：拼音中途提交、退格、emoji+CJK、候选方向键、组合态 Ctrl+Enter 不误发。
- [ ] 预览与附件：安全文件名、图片/CID、文本/HTML、滚动、焦点恢复、保存状态。
- [x] 同步/通知：真实业务状态转移有离线模拟验证，不使用真实账号跑回归（E5，2026-09-11）。
- [x] 深浅/自定义主题、多窗口更新、标准/紧凑/极简工作区的现有离屏截图集审查（2026-09-14，83 张；不代表真实 OS、多 DPI 或代表性大图验收）。
- [ ] release 体积、运行内存与大列表性能实测，对比可用基线；不虚构预算数字。（E9 实测已完成，见 2026-09-20 记录；预算数字待定义，故不勾选）
- [x] 删除 spike 及旧控件残留，更新 CHANGELOG，复核并锁定 Cargo.lock，再按授权完成提交（2026-09-20，仅 commit 未 push、未正式打包）。

## 本轮证据记录

2026-09-10：E1 基础接入与 E2 本轮列出的控件已落地；整体目标仍未完成。

| 最终自动检查 | 结果 |
| --- | --- |
| 主程序 check / build | 通过 |
| 原有 lib / main 单测 | 46 + 41 = 87 项通过 |
| 原生 GPUI 回归 | 47 项通过 |
| 主程序隔离自动退出 | exit 0，4164 ms |

原生检查包括：Root/SVG 资源、Onboarding 与写信的模拟 Tab、Dialog/Sheet/Notification
实际绘制、设置 Input/Switch/TabBar 实际绘制、HEX 模拟 Tab 不丢草稿、普通 Enter
仅换行不发送、生产 Change 订阅保存草稿以及重新打开写信窗口恢复草稿。

窗口始终隐藏且不聚焦 OS 桌面；这是实际 GPUI 状态、绘制和合成按键的证据，
不是原生 IME 或完整截图验收。调用的发送入口没有执行；写信测试收件人保持为空。

本轮未修改依赖或锁文件，也未清理 spike、暂存或提交。
保留原有两个 dead_code 警告，当前没有把警告清理当作迁移成果。

日志与前后快照：`C:/Users/lzzwf/Desktop/foxmail-ecosystem-20260910-220002/`。
根目录的 `verify.ps1` 可重复验证；其中一次性 apply/correction 脚本不可重复执行。

以上为 2026-09-10 的阶段记录。服务层测试通过不等于对应业务已迁移，
也不得以基础接入完成为由提前关闭完整生态目标；后续进度见下节。

## 2026-09-11：E3/E4 业务接入与有效视口验收

本批实现：

- 配色编辑器采用 `Select`、3 个 `Slider`、`Form/Field`；可编辑项仍为
  `Primary / Canvas / Surface / Text / Border`，没有扩大或削减现有主题模型。
  HEX/选择器/Slider 的程序同步不发业务编辑事件；HSV 独立保存，经过灰色/黑色后
  恢复饱和度/亮度不会丢失色相。库 Slider 缺少 `Focusable`，用薄键盘适配层补齐
  Tab、方向键、Home/End，不再保留旧的 HSV 拖拽实现。
- 设置与附件预览使用实际 `Root` Dialog、正式 `DialogContent`。独立的被观察内容视图
  避免在 `MailApp::render` 构建浮层时重新借用主实体。保存、取消、外部点击、Esc、
  小窗口布局、焦点恢复、保存失败保留草稿均有业务回归。预览取消/索引失效会推进
  generation，迟到的图片加载不能重开浮层。
- 搜索使用受控 `Popover` 和 `ListItem`，保留最近邮件与真实离线索引查询。
  专用搜索 action 避免 Popover 的 Enter 提前清空选择；在 Input 上下文解绑
  Popover 的 Space，空格仍交给官方 Input 编辑。方向键滚动保持选中项可见。
  marked-text 防误选只属于合成输入证据，**不是 Windows 拼音 IME 验收**。

测试基础设施修正：

- `native-ui-tests` 显式启用 `gpui-kit/test-support`，普通产品构建不启用它。
  Cargo.lock 仅增加 10 个必要测试依赖，原有包没有移除或升级；Kit/Base/Component
  仍为 0.6.1，GPUI/Platform 仍为 0.3.4，仍是单栈。
- Windows 隐藏窗口初始化时可能仍保留 `CW_USEDEFAULT` 尺寸，旧夹具会生成 1×1
  纹理。新夹具先显式 resize，再断言有效视口；退化截图直接失败。
  2026-09-10 的记录不能作为有效尺寸截图/视觉证据引用，原有状态/事件检查本批重跑。
- 测试应用使用显式退出模式，关闭业务浮层、排空事件/动画、移除测试窗口并释放句柄
  后才退出。保留官方 leak-detection；输出 PASS 后若进程非零退出仍判失败。
- 新测试 helper 放在 `tests/support/`，不让 Cargo 将其自动收集为独立测试。
  每个新增场景注明状态、合成按键/指针、离屏截图等类别；报告将截图场景单独计数。
- 四种主题 × 标准/紧凑/极简工作区，以及设置/搜索/预览/已打开写信窗口的
  Windows DirectX 离屏截图均保留。截图生成和布局/像素检查不等于视觉审阅。
  本会话图像查看工具因 `SetTokenInformation(TokenDefaultDacl) failed: 1344`
  无法打开图片，`visual_review_complete` 保持 false，人工/工具视觉审阅仍是门禁。
- 锁定版本的 Base Button 明确记录 GPUI 尚无 `aria-disabled` setter；禁用路径按
  实际点击不可保存、校验状态与截图验证，不伪造已完成原生可访问性审计。

证据目录：`C:/Users/lzzwf/Desktop/foxmail-e3-e4-20260911-084135/`。
可重复执行的 `verify.ps1` 覆盖四条自动门禁及独立主程序冒烟；`implement.py`
是一次性、带前置哈希校验的应用脚本，不应再次执行。

| 本批最终自动门禁 | 结果 |
| --- | --- |
| `cargo check --locked --offline --bin foxmail-rs` | 通过 |
| lib / main 单测 | 46 + 41 = 87 项通过 |
| 原生 GPUI 显式 feature 回归 | 96 条记录，进程 exit 0，无句柄泄漏报告 |
| 回归记录分类 | 68 个状态/事件/布局检查 + 28 个截图/跨窗场景 |
| `cargo build --locked --offline --bin foxmail-rs` | 通过 |
| 主程序独立隔离自动退出 | exit 0，4064 ms，自动退出配置 3000 ms |
| 截图文件 | 30 张有效尺寸、非空 PNG；含 2 张设置回归截图，视觉审阅未完成 |
| 默认构建 GPUI 反向依赖树 | 单一 GPUI 0.3.4；未改现有包版本 |

最终日志：`final-check.log`、`final-unit-tests.log`、`final-native.log`、
`final-build.log`、`final-gpui-tree.log`、`final-smoke.json`。
分类报告副本：`final-native-result.json`；截图入口：`screenshots.html`，
像素/尺寸/哈希清单：`screenshot-metrics.json`。保留原有两处 dead_code 警告，
没有将冒烟耗时当成 release 性能预算。


**该批之后的推进顺序为 E5 同步/保存/发送通知、E6 导航/列表/滚动/分隔、E7 附件/正文，
随后完成 E8/E9 全量验收；E5 的后续完成记录见下节。**

原生 OS IME（含候选方向键、emoji+CJK、组合态 Ctrl+Enter）、截图审阅、托盘与
跨窗口 OS 焦点、release 体积/内存/大列表预算、spike 与旧控件清理、CHANGELOG、
最终锁文件复核和按授权提交均未从范围删除，也未因本批自动回归通过而打勾。
本批不暂存、不提交，不清理尚未完成的 T5 内容。

## 2026-09-11：E5 业务反馈与离线回归

本批完成的是 E5 的业务接入及自动门禁，不是整体迁移、原生 OS 交互或视觉验收完成。

### 实现

- 新增窗口级 `feedback` 薄适配层，使用官方 `Alert / Progress / Notification`。
  通知由业务状态转移入队，不在重绘时重新推导；稳定 ID 按窗口实体及业务通道替换。
  同步、附件、设置、发送、草稿相互独立；明确指定 `InApp`，不继承系统 toast 模式。
  成功／普通信息自动收起，错误／警告保留至关闭；长状态区限制高度并允许滚动。
- 同步区分下载、`Indexing` 与真正完成；入库及重载均成功后才报告成功。
  generation 拒绝迟到／重复回调，busy 阶段不可重入。空轮询不误报失败，但工作线程
  无终端事件断开会结束等待并报告失败。连续邮件保持邮箱上下文，断点续传从实际
  resume 位置显示，旧 UID 的进度不污染当前邮件；未知大小不伪装成 100%。
- 关闭同步 Alert 只隐藏状态，不取消或解锁任务；原有 `Ctrl+Shift+S` 可恢复同一任务。
  自动无新邮件不重复吐成功通知，也不重载、清空当前阅读选择及分页。
  自动未配置账号保持安静，手动请求明确说明原因；重新输入账号使旧回调失效。
- 附件保存继续使用真实文件对话框之后的共同业务路径。取消不报成功；保存期间不
  重入，owner/message 与 generation 保证切换邮件后不会把旧结果贴到新邮件上。
  读取与写入错误分开提示／记录：目标位置不可写不再提示“索引结构不完整”。
  日志仅保留结构化错误指纹，不写出错误中的完整路径、正文或凭据。
- 设置只在持久化及应用成功后报告已保存。非法 HEX／取消不通知成功；相同编辑
  session 的重复保存错误不刷屏，失败仍保留设置草稿，恢复后可正常重试。
- 写信使用发送代次与 `DraftData` 快照；发送失败保留草稿，成功只清理未改变的
  已发送快照，发送期间及另一写信窗口的新修改不会被删除。草稿清理失败仍是
  “邮件已发送”并提示不要重复发送，不能误标 SMTP 失败。窗口关闭后的异步成功
  也只清理一致快照，失败保留草稿。正文仍由普通 Enter 换行，组合态 guard 保留。
- 实际 Input `Change` 自动保存错误现在可见，同错误通知去重；写入恢复只通知一次，
  正常逐字编辑不反复发成功通知。未改变草稿 JSON schema 或引入新的依赖。

### 回归与证据

新增 `tests/support/gpui_feedback.rs`、`tests/support/gpui_send_feedback.rs`。
同步使用本地 `mpsc` 事件和真实隔离 SQLite／EML 导入；发送只调用 `begin_send`
及注入本地 Task 的 `dispatch_send`，不调用带非空收件人的真实 `attempt_send`，
不建立 IMAP／SMTP 连接。Windows 草稿清理失败用允许读取但禁止删除的文件句柄
制造真实删除错误；附件／设置／自动草稿保存失败也实际操作隔离文件系统。

| 本批最终自动门禁 | 结果 |
| --- | --- |
| `cargo check --locked --offline --bin foxmail-rs` | 通过 |
| lib / main 单测 | 46 + 48 = 94 项通过（本批新增 7 项） |
| 原生 GPUI 显式 feature 回归 | 146 条记录，进程 exit 0，无句柄泄漏报告 |
| 全部原生回归分类 | 103 个状态／事件／布局检查 + 43 个截图场景 |
| E5 新增回归分类 | 35 个状态／事件／布局检查 + 15 个截图场景 = 50 条 |
| `cargo build --locked --offline --bin foxmail-rs` | 通过 |
| 主程序独立隔离自动退出 | exit 0，4258 ms，自动退出配置 3000 ms |
| 截图文件 | 45 张有效尺寸、非空 PNG；其中 15 张为 E5；视觉审阅未完成 |
| 默认构建 GPUI 反向依赖树 | 单一 GPUI 0.3.4；Kit / Base / Component 0.6.1 |
| 本批依赖与编码检查 | Cargo.toml / Cargo.lock 未改；UTF-8／既有 CRLF/LF 保留 |

证据目录：`C:/Users/lzzwf/Desktop/foxmail-e5-20260911-110845/`。
`verify.ps1` 可重复执行四条门禁、单栈检查和独立主程序冒烟；
`collect-evidence.py` 汇总报告、源码哈希与 E5 相对前置备份的补丁。
本批 `implement-*.py`、`extend-e5-unit.py`、`refine-e5-errors.py` 是一次性应用脚本，
不要再次执行覆盖当前工作区。

最终日志为 `final-check.log`、`final-unit-tests.log`、`final-native.log`、
`final-build.log`、`final-gpui-tree.log`、`final-smoke.json`。
`final-native-result.json` 保留全部分类，`e5-native-result.json` 仅包含本批新增记录。
`screenshots.html` 是截图入口，`screenshot-metrics.json` 记录尺寸／像素／哈希；
两张原有设置截图附着于状态用例，因此 PNG 数量比独立截图场景多 2 张。

图像查看工具本批仍因 `SetTokenInformation(TokenDefaultDacl) failed: 1344`
无法打开 PNG；`visual-review-attempt.json` 保留失败记录，
`visual_review_complete` 继续为 false。合成 marked-text／Ctrl+Enter 检查不是
Windows 拼音候选窗验收。主程序冒烟耗时也不是 release 性能预算。

**下一主攻为 E6 导航／列表／滚动／分隔，再做 E7 附件／正文和 E8/E9 全量验收。**
原生 IME、截图视觉审阅、托盘与跨窗口 OS 焦点、release 体积／内存／大列表预算、
spike／旧控件清理、CHANGELOG、最终锁文件复核和按授权提交均继续保留。
本批仅勾选已有证据支持的“同步／通知离线验证”，其余完整验收门禁未提前打勾；
不暂存、不提交、不清理 T5 内容。

## 2026-09-14：E6/E7 回归续验、E8 视觉修复与 E9 预发布测量

上午 E6/E7 的实现与自动回归已完成一批验证（证据：
`C:/Users/lzzwf/Desktop/foxmail-e6-e7-resume-20260914-085812/`）：
48 + 58 = 106 项单测、170 条原生记录，不能再把 E6/E7 表述为“尚未开始”。
但那批截图尚未打开审阅，不能据此宣称完整交付。

### 本批实际修复

1. `src/app.rs`：官方 ListItem 将 children 包在 block 容器中，直接并列放置
   Icon/Label/Count 导致展开侧栏溢出固定行高；改为单个横向 flex 内容节点，
   标签截断、图标/计数不收缩，并用原生元素 bounds 验证不重叠。
2. `src/ui.rs`：默认 Assets 仅带基础图标。新增按需资源源，补入 Download、
   FilePen、Forward、Image、Paperclip、Reply、Send、ShieldAlert、SquarePen、
   Tags、Trash、X；仍保留官方默认资源。不启用全量 1,800+ 图标包。
   单测从实际 MailIcon 调用点枚举资源，避免“枚举可编译但 SVG 未打包”。
3. `src/fox_theme.rs`：颜色字段、Component resolved tokens 与 Base 投影
   从同一配色更新。补齐列表选中态、进度条及滑块映射。写信窗口回归检查
   实际发送按钮区域的主色像素，不再仅检查 view/theme 里的字段值。

新增 `tests/support/gpui_acceptance.rs`，显式 `--suite E8` 可独立运行。
完整套件也执行 E8，覆盖四种主题、展开/折叠侧栏、窄窗口列表/侧栏/正文、
极简工作区和已打开写信窗口。此类输入/布局证据不是 Windows OS 输入法验收。

### 自动门禁与截图审阅

- 单元测试：48 + 60 = **108 项通过**。
- 完整原生回归：**226 条通过**，145 个状态/事件/布局检查 + 81 个截图场景；
  E8 新增 24 + 32 = 56 条；另有 2 张嵌在既有状态用例中的截图，总计 83 张 PNG。
- 本批 view_image 可正常工作。已查看全部 83 张的联系表并放大抽查关键截图，
  确认侧栏重叠、图标漏显与主色不同步三类问题修复。不是仅检查文件非空。
- `final-native-result.json` 是未经改写的自动报告，仍保留生成时
  `visual_review_complete=false`；实际审阅单独保存在 `visual-review.json`，
  包含图像 SHA-256、覆盖范围、抽查名单及未完成项，避免改写原始证据。
- 现有 tiny GIF 的预览/CID 用例不能替代具有代表性的大图/大附件视觉验收。

### 独立 release 候选版实测（不是正式发布）

候选版路径：`target/e8-review-20260914/release/foxmail-rs.exe`。
使用 `--locked --offline --release` 在独立 target 目录构建，不覆盖
`target/release/foxmail-rs.exe`，也未变更桌面快捷方式。

使用同一份 10,007 封虚构邮件、4 个 `.invalid` 账号的 SQLite 索引，
旧版与候选版交替各启动 3 次；配置/凭据/日志完全隔离，自动退出设置为 6 秒。
六次 exit 0，采样中未发现不响应。不连接 IMAP/SMTP，不进行正文/附件读取操作。

| 指标 | 9 月 9 日旧 release | 本批候选版 |
| --- | --- | --- |
| EXE 大小 | 17,685,504 B / 16.87 MiB | 31,666,688 B / 30.20 MiB |
| 每次峰值工作集范围 | 65.73–65.85 MiB | 69.19–69.23 MiB |
| 每次峰值 private bytes 范围 | 77.57–77.71 MiB | 73.12–73.18 MiB |

内存为 100 ms 间隔采样的观测峰值；这是初始索引窗口，不是滚动/正文/附件性能测试。
旧版和候选版依赖栈不同，体积差不能归因于本批 12 个图标；冷/热启动也有差异，
不能据此笼统宣称内存优化。未给出新预算，原完整性能/体积验收门禁不提前勾选。

### 证据与剩余工作

证据目录：`C:/Users/lzzwf/Desktop/foxmail-e8-e9-20260914-165308/`。
主要入口：`final-gates.json`、`final-native-result.json`、`visual-review.json`、
`screenshots.html`、`release-metrics.json`、`source-after.json` 与本批补丁。
已有源码改动未覆盖，Cargo.toml/Cargo.lock 未更改，未暂存、未提交、未正式打包。

**下一主攻：按 `GPUI-NATIVE-ACCEPTANCE.md` 验证真实 IME、OS 焦点/托盘和 DPI；
补代表性图片/大附件及 release 滚动/长时间运行验收，再做最终清理与按授权提交/发布。**
SMTP 主机及发件配置仍沿用 PoC 常量；新增 IMAP 账号不等于通用 SMTP 多账号已完成，
后续产品化前需单独处理，不用真实账号做本轮自动验收。

## 2026-09-20：E9 续测——10k 启动内存与 10 分钟驻留

独立候选版（`target/e9-20260920/release/xiou.exe`，SHA-256
`86A90DAE…4C19CE35`，30.86 MiB，`--locked --offline --release`）配 10,007 封
虚构邮件（4 个 `.invalid` 账号、5 个邮箱，含 100 封约 200 KB 长正文与 20 封
约 1 MB 内联 PNG），经应用自身 `import-eml` 导入。隔离
LOCALAPPDATA/APPDATA/TEMP 与索引/备份环境变量；采样间隔 100 ms。证据与工具：
`C:/Users/lzzwf/Desktop/foxmail-e9-20260920/`（`e9-summary.md`、
`startup-result.json`、`dwell-result.json`、`measure.ps1`、`gen-corpus.py`）。

- 启动窗口（AUTO_EXIT_MS=6000，3 次均 exit 0、无挂起）：峰值工作集
  85.38–89.58 MiB（一次离群 76.95）、峰值 private bytes 78.35–85.69 MiB。
- 无账号配置（onboarding）对照 2 次：89.10 MiB WS、85.57 MiB private，与
  主界面+同步泵几乎相同，差异不来自后台同步重试。
- 10 分钟驻留：全程无挂起，WS 85.0→76.4 MiB 平台期、private 80.4→80.8 MiB
  平稳，无泄漏趋势；峰值仍在启动期。
- 与 9 月 14 日候选（69.19–69.23 MiB WS / 73.12–73.18 private）相比上升
  16–20 MiB（private +5.2–12.5），但依赖栈与界面规模均不同，不能逐项归因；
  不设定新预算。
- 边界不变：本批只覆盖启动窗口与空闲驻留，不含滚动/搜索/长正文/弹层操作
  中的延迟与内存；N7 对应人工/交互项仍未完成，E9 门禁不提前勾选。

## 2026-09-20：真实 OS 交互验收——用户人工结论

同日补齐 Media 大图/附件语料（4032×3024 内联 CID、1080×1920 竖屏 PNG、
4K JPEG 附件、2MB PDF、5MB 大附件、缺失 CID，经 import-eml 入隔离索引，
总 10,013 封），并在隔离实例上做了真实 OS 交互尝试（注入工具旁证 + 截图）。
用户直接操作后给出总体结论："现在这个软件没有什么问题"。

- 按 `GPUI-NATIVE-ACCEPTANCE.md` 的 N1–N7，记录为**用户人工使用验收无异常，
  未逐项留证**；完整记录见
  `C:/Users/lzzwf/Desktop/foxmail-e9-20260920/native-acceptance-20260920.md`。
- 输入注入旁证：键盘导航生效；OS 鼠标点击一次成功（选中西发送）后注入
  点击被应用忽略（悬停正常），根因未查，工具与限制已记录在案。
- 剩余未完成：release 体积/内存/大列表性能预算对比（需先定预算）、
  spike/旧控件清理、Cargo.lock 复核、按授权提交与正式打包。

## 2026-09-20：spike/旧控件清理与原生门禁补跑

用户授权后完成的收尾批次（提交为本批最后一项，只 commit 不 push）：

- 删除 `src/spike_gpui_kit.rs`（293 行组件评估演示，未跟踪文件，主程序已全量
  迁移其覆盖的组件，无独有逻辑）并移除 `Cargo.toml` 中 `spike-gpui-kit` 的
  `[[bin]]` 条目。至此 spike/旧控件残留全部清零：`composer_editor.rs` 此前已删，
  `foxmail-rs.ico` → `xiou.ico` 重命名已完成（`tray.rs` 引用 `xiou.ico`）。
- Cargo.lock 复核：2,380 插入 / 451 删除全部来自 GPUI 迁移依赖树；
  `--locked --offline` 全部门禁通过，锁文件与 Cargo.toml 一致。
- 修正迁移文档中的门禁命令：bin 已更名 `xiou`，旧 `--bin foxmail-rs` 已失效。
- **补跑原生门禁时发现并修复一个 9 月 18 日引入的真回归**：E9 批次只做了
  测量与 OS 验收，未重跑原生套件；2026-09-14 之后 `app.rs`/`mail_body.rs`
  的显式图片尺寸改动引入 `overflow_x_scrollbar()`，而 gpui-component 的
  `Scrollable` 会把被包裹元素的 id 改写为 `(调用点位置, "content")` 复合 id，
  导致 `mail-cid-image` 测试标记从元素树消失，E7 CID 断言确定性失败
  （`tests/support/gpui_mail_content.rs` 曾报 "missing CID render marker"）。
  修复：将 `mail-cid-image` 标记 id 移到 `Scrollable` 包装之外的外层元素
  （`app.rs` Image 分支），横向滚动结构与渲染尺寸不变；同步为缺失 CID
  fallback 补 `w_full().min_w_0()`（与原外层 wrapper 布局等价）。
  诊断方法：临时在断言前 dump 全量 element 快照定位复合 id，定位后已还原。

| 清理后最终自动门禁（2026-09-20） | 结果 |
| --- | --- |
| `cargo check --locked --offline --bin xiou` | 通过（5 个既有 dead_code 警告，与本批无关） |
| `cargo test --locked --offline --lib --bin xiou` | 48 + 63 = 111 项通过 |
| 原生 GPUI 显式 feature 回归 | 226 项全部通过（与 2026-09-14 基线数量一致），exit 0 |
| `cargo build --locked --offline --bin xiou` | 通过 |

本批不改变任何产品行为语义（仅测试标记 id 位置与此前 spike 删除）；
release 体积/内存预算仍待用户定义数字，正式打包仍待另行授权。
