# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.7.3] - 2026-06-10

### Added

- **Limit Survival Forecast**: Added an overview survival card with source-aware quota lookup for Claude and GPT subscriptions so remaining runway is visible directly from the dashboard
- **Usage Share Posters**: Added themed usage poster export from the statistics view for sharing current usage snapshots
- **Recent Requests Panel**: Added recent request activity in sessions so the latest requests are visible alongside session details

---

### 新增

- **限额生存预测**：概览面板新增限额生存卡，并支持面向 Claude 与 GPT 订阅的来源额度查询，可直接在仪表盘查看剩余可持续时间
- **用量分享海报**：统计视图新增主题化用量海报导出能力，便于分享当前用量快照
- **最近请求面板**：会话视图新增最近请求活动展示，可在会话详情旁直接查看最新请求

---

### Fixed

- **macOS Tray Interaction**: Fixed macOS menu bar interaction and unified app exit handling for more reliable tray behavior and quitting

---

### 修复

- **macOS 菜单栏交互**：修复 macOS 菜单栏交互并统一应用退出流程，使托盘行为和退出流程更稳定

---

## [0.7.2] - 2026-06-06

### Added

- **WSL Passive Scanning**: Added WSL passive scanning with per-distribution toggles so UsageMeter can collect local usage data from supported WSL environments
- **WSL UI Markers**: Added WSL-aware labels in sessions and projects so WSL-sourced records are visible in the UI
- **WSL OpenCode Coverage**: Extended OpenCode local usage scanning to work inside WSL

---

### 新增

- **WSL 被动扫描**：新增 WSL 被动扫描，并支持按发行版独立开关，使 UsageMeter 可以从受支持的 WSL 环境采集本地用量数据
- **WSL 界面标记**：会话和项目现在会显示 WSL 感知标签，方便在界面中识别来自 WSL 的记录
- **WSL OpenCode 覆盖**：扩展 OpenCode 本地用量扫描，使其可在 WSL 内工作

---

## [0.7.1] - 2026-06-06

### Added

- **Reasonix Support**: Added Reasonix local session metadata parsing and proxy takeover support so UsageMeter can track Reasonix usage alongside Claude Code, Codex, and OpenCode
- **Session & Project Coverage Views**: Added coverage status views for sessions and projects so it is clear which records are tracked after proxy takeover

### Changed

- **Unified Post-takeover Statistics**: Unified statistics and coverage status display after takeover, with agent-record backfill matching so historical local sessions are reconciled with proxy records
- **OpenCode Config Source Tracking**: Track the effective OpenCode configuration source and patch the takeover configuration in place to reduce conflicts
- **Proxy Takeover Recovery**: Improved proxy takeover recovery handling for more reliable resume after configuration changes

### Fixed

- **Reasonix Icons**: Fixed the Reasonix icon in the tool selector and the overview breakdown

---

### 新增

- **Reasonix 支持**：新增 Reasonix 本地会话元数据解析与代理接管支持，使 UsageMeter 可以与 Claude Code、Codex、OpenCode 一并追踪 Reasonix 用量
- **会话与项目覆盖率视图**：新增会话与项目的覆盖状态视图，清晰展示代理接管后哪些记录已被纳入统计

### 变更

- **接管后统一统计**：统一接管后的统计与覆盖状态展示，并补全代理记录回填匹配，使历史本地会话与代理记录得以对齐
- **OpenCode 配置源追踪**：追踪生效的 OpenCode 配置源，并就地修补接管配置以减少冲突
- **代理接管恢复**：增强代理接管的恢复处理，使配置变更后的接管恢复更稳定可靠

### 修复

- **Reasonix 图标**：修正工具选择器与概览细分中的 Reasonix 图标

---

## [0.7.0] - 2026-06-04

### Added

- **OpenCode Support**: Added OpenCode local session scanning and proxy takeover support so UsageMeter can track OpenCode usage alongside Claude Code and Codex
- **Cache Hit Rate Metric**: Added a cache hit rate indicator in statistics to expose local cache effectiveness directly in the UI

### Changed

- **Theme System Refresh**: Reworked the global theme system to unify accent colors, shared control styling, and hardcoded color cleanup across the tray UI
- **Settings Panel Layout**: Reorganized the settings view into a denser, clearer layout with improved local cache management controls and better use of the compact tray surface
- **Usage Backend Structure**: Refactored usage, local database, proxy, and session modules into smaller bounded components to reduce coupling and make future feature work safer

### Fixed

- **Mixed-source Status Visibility**: Fixed statistics so mixed local/proxy results no longer hide status code information and instead mark local-only requests with a dedicated badge

---

### 新增

- **OpenCode 支持**：新增 OpenCode 本地会话扫描与代理接管支持，使 UsageMeter 可以与 Claude Code、Codex 一并追踪 OpenCode 用量
- **缓存命中率指标**：统计面板新增缓存命中率指标，可直接在界面中查看本地缓存的实际生效情况

### 变更

- **主题系统刷新**：重构全局主题系统，统一 accent 色、共享控件样式，并清理托盘界面中的硬编码颜色
- **设置面板布局**：重新整理设置页为更紧凑清晰的布局，增强本地缓存管理控件，并更好适配托盘窗口的紧凑空间
- **用量后端结构**：将 usage、本地数据库、代理与会话模块拆分为更小的边界组件，降低耦合，为后续功能演进提供更稳妥的结构基础

### 修复

- **混合来源状态可见性**：修复本地与代理混合来源统计会隐藏状态码信息的问题，改为通过专用 Local 标记展示仅本地请求

---

## [0.6.4] - 2026-06-01

### Added

- **Theme Palette Selector**: Added a richer theme system with palette selection and gradient surfaces across the tray UI
- **Transparent Proxy Takeover Safeguards**: Added conflict detection so proxy takeover pauses when external Claude/Codex configuration changes would clash with UsageMeter-managed settings

### Changed

- **Theme-aware Interface Refresh**: Updated overview, statistics, sessions, pricing, sync, currency, and update surfaces to use shared theme variables and more consistent semantic coloring
- **Local Proxy Architecture**: Refactored the local proxy into a transparent forwarding path, simplifying request routing and reducing configuration-specific branching

### Fixed

- **Updater Interaction Flow**: Replaced the update banner with a dialog and refined skip-version behavior between automatic and manual update checks
- **Codex Streaming Timeout**: Fixed Codex streaming requests failing immediately when `read_timeout(0)` was treated as an instant timeout
- **Proxy Config Rewrite Noise**: Removed redundant config rewrites during external Codex config sync when proxy takeover was already active

---

### 新增

- **主题调色板选择器**：新增更完整的主题系统，为托盘界面提供调色板切换与渐变表面样式
- **透明代理接管保护**：新增外部配置冲突检测，当 Claude/Codex 配置与 UsageMeter 托管设置冲突时会暂停代理接管

### 变更

- **主题感知界面刷新**：统一概览、统计、会话、定价、同步、货币和更新相关界面的主题变量与语义色彩表现
- **本地代理架构**：将本地代理重构为透明转发链路，简化请求路由并减少针对特定配置的分支处理

### 修复

- **更新交互流程**：将更新提示从横幅改为弹窗，并优化自动检查与手动检查之间的跳过版本逻辑
- **Codex 流式超时问题**：修复 `read_timeout(0)` 被当成立即超时后导致 Codex 流式请求瞬时失败的问题
- **代理配置重复回写**：移除代理接管已启用时外部 Codex 配置同步中的冗余配置回写

---

## [0.6.3] - 2026-05-30

### Added

- **Unified Daily Materialization Layer**: Added materialized daily fact, summary, model-summary, and state tables so historical unified usage can be reused without recomputing every statistics view
- **Performance Diagnostics**: Added merge/materialization performance logs and dependency snapshots to inspect history rebuilds, cache hits, and hot-day read costs

### Changed

- **Statistics Aggregation Path**: Statistics summary, month activity, and year activity now prefer cached historical summaries with a real-time hot-day overlay, significantly reducing CPU overhead during repeated refreshes and time-range switches
- **Request Identity and Invalidation**: Unified facts now carry canonical request keys, and proxy record updates now participate in `updated_at`-aware invalidation for historical materialization caches

### Fixed

- **Partial Coverage Semantics**: Status and performance coverage are now marked partial only when proxy-backed and local-only data are mixed, preventing valid metrics from being blanked unnecessarily
- **Local Day Boundary Calculation**: Local day epoch bounds now resolve against timezone-aware day boundaries instead of a fixed 24-hour span, avoiding DST-related day splits
- **Activity Grid Hover Sync**: Month and year activity tooltips now stay synchronized when activity data refreshes while a day is hovered

---

### 新增

- **统一每日物化聚合层**：新增每日事实表、汇总表、模型汇总表以及物化状态表，使历史统一用量数据可以直接复用，不必在每次统计视图中重复全量计算
- **性能诊断信息**：新增合并/物化性能日志与依赖快照，便于观察历史重建、缓存命中以及热点日读取成本

### 变更

- **统计聚合路径**：统计摘要、月度活动和年度活动现在优先复用历史汇总数据，并叠加实时热点日数据，在重复刷新和切换时间范围时显著降低 CPU 开销
- **请求标识与失效机制**：统一事实现在携带规范化请求键，代理记录更新也会参与基于 `updated_at` 的历史物化缓存失效判定

### 修复

- **部分覆盖语义**：状态和性能覆盖现在仅在代理数据与仅本地数据混合时标记为部分覆盖，避免本来有效的指标被不必要地清空
- **本地日边界计算**：本地日期边界不再使用固定 24 小时跨度，而是按时区感知的自然日边界计算，避免夏令时场景下的跨天错算
- **活动网格悬停同步**：当活动数据刷新时，月视图和年视图的 tooltip 悬停状态现在会保持同步

---

## [0.6.2] - 2026-05-27

### Changed

- **Usage Refresh Pipeline**: Optimized the usage data refresh pipeline to reduce CPU overhead, removing redundant local query paths during time-range switches
- **Settings View Structure**: Refactored oversized settings views into smaller, more maintainable components

### Fixed

- **Settings Migration Logic**: Consolidated settings migration logic and removed hardcoded proxy mode markers that could cause stale state
- **Proxy Settings Cache**: Settings are now cached in-memory at runtime, eliminating per-request file I/O that degraded proxy throughput
- **Proxy State on Exit/Update**: Exiting or restarting for an update no longer clears the proxy takeover enabled flag — only external provider config is restored

---

### 变更

- **用量刷新链路优化**：优化用量数据刷新管线以降低 CPU 开销，移除切换时间范围时冗余的本地查询路径
- **设置视图结构**：将过大的设置视图拆分为更小、更易维护的组件

### 修复

- **设置迁移逻辑**：收口设置迁移逻辑，移除可能导致陈旧状态的硬编码代理模式标记
- **代理设置缓存**：设置现在在运行时缓存到内存，消除每次请求的文件 I/O 对代理吞吐量的影响
- **退出/更新时代理状态**：退出或更新重启时不再清除代理接管开关状态，仅恢复外部供应商配置

---

## [0.6.1] - 2026-05-26

### Removed

- **`LocalOnly` Data Source Mode**: Removed the redundant `LocalOnly` data source setting — `ProxyWithLocalFallback` already automatically falls back to local file statistics when no proxy data is available, making the manual toggle unnecessary

### Fixed

- **Update Checker Reliability**: Extracted shared `build_updater` helper so both background startup checks and manual checks use the same proxy-aware updater instance — fixes cases where one path ignored proxy settings
- **Update Banner with Error State**: When an update is detected but the download fails, the update banner now remains visible instead of disappearing with the error state
- **Update Error Message Clarity**: Update check failures now surface a localized `checkFailed` key instead of a raw error string, preventing untranslated technical messages from appearing in the UI

---

### 移除

- **`LocalOnly` 数据源模式**：移除冗余的「仅本地文件」数据源选项——`ProxyWithLocalFallback` 已在无代理数据时自动回退至本地文件统计，用户无需手动切换，该选项对用户而言是多余的

### 修复

- **更新检测可靠性**：抽取 `build_updater` 公共函数，确保启动后台检测与手动检测均复用同一套代理感知逻辑，修复了某些路径下未注入代理配置的问题
- **有更新时错误状态下横幅消失**：下载失败进入错误状态时，若已检测到新版本，更新横幅现在会持续显示而非随错误状态消失
- **更新检测错误信息显示**：更新检测失败现在使用本地化 Key `checkFailed`，不再将原始错误字符串直接渲染到界面

---

## [0.6.0] - 2026-05-25

### Added

- **Local Cache Persistence**: Parsed Claude / Codex session facts now persist in `local_usage.db` and survive deletion of source JSONL files — statistics never drop just because you cleaned up old sessions
- **Local Cache Management UI**: New section in Settings → Data shows total facts and orphan facts (rows whose JSONL is gone), with one-click _Purge orphans_ (with 30 / 90 / 180 day or all-time retention) and _Rebuild cache_ actions
- **Source Label on Merged Facts**: Merged request facts now carry a `source_label` derived from `api_key_prefix` or `request_base_url`, enabling honest "unidentified source" bucketing for proxy-bypassed local requests

### Changed

- **Per-field Merge Priority**: The proxy/local merge logic now picks priorities per field category instead of a blanket "proxy wins" rule:
  - `input/output_tokens` → proxy preferred (most authoritative)
  - `cache_create/cache_read_tokens` → **local preferred** (JSONL parsing is more complete than streaming SSE)
  - `total_tokens` → **recomputed from parts** to avoid drift when either side under-reports
  - `estimated_cost` → proxy when `cost_locked = true`, otherwise local real-time estimate so price-table edits take effect immediately
  - status / duration / TTFT / rate → **proxy only**
- **Source Filter No Longer Cuts Local Fallback**: When you filter by an API source, proxy-recorded requests of other sources are still excluded, but truly proxy-missed local requests are kept (bucketed as "unidentified source") — previously the entire local fallback was dropped
- **Soft-delete Semantics**: `sync_from_scanner` now soft-deletes vanished sessions (mark `source_file_present = 0`) instead of physically removing rows, so history is preserved while the source-file table still tracks current filesystem state

### Fixed

- **Cross-device Sync Double-counting**: Outbox `request_key` and `event_id` now use the same normalized global key (`tool:message_id` / 9-tuple fallback) as the local table, fixing duplicate row creation in `remote_request_facts` after WebDAV import
- **Schema v5 Upgrade Crash**: Old databases (v4) used to fail at startup with `no such column: deleted_at in CREATE INDEX ...` because v5 indexes were declared in the table-creation batch before the migration could add the columns; indexes are now created only inside the v5 migration branch
- **`/clear` Context Not Erasing History**: When users clear in-session context inside Claude, the previously seen `message_id`s are now soft-marked instead of deleted — historical statistics no longer drop unexpectedly

---

### 新增

- **本地缓存持久化**：解析过的 Claude / Codex 会话事实现在持久化到 `local_usage.db`，即便原始 JSONL 被删除统计也不会消失——清理旧会话不再让历史数字下降
- **本地缓存管理 UI**：设置 → 数据与配额下新增本地缓存管理区，显示事实记录数与孤立记录数（来源 JSONL 已消失的行），并提供一键「清理孤立记录」（支持 30 / 90 / 180 天或全部窗口）与「重建缓存」按钮
- **合并事实的来源标签**：合并请求事实新增 `source_label` 字段，从 `api_key_prefix` 或 `request_base_url` 派生，让代理未覆盖的本地请求可以诚实归入「未识别来源」桶

### 变更

- **字段级合并优先级**：代理与本地合并不再统一「代理优先」，改为按字段类别分桶：
  - `input/output_tokens` → 代理优先（响应头/body 最权威）
  - `cache_create/cache_read_tokens` → **本地优先**（JSONL 解析比流式 SSE 拿到的更全）
  - `total_tokens` → **按各字段重新计算**，避免任一方少算导致漂移
  - `estimated_cost` → `cost_locked = true` 时用代理冻结值，否则用本地实时估算（修改价格表后立即生效）
  - 状态码 / 耗时 / TTFT / 速率 → **仅代理**
- **来源过滤不再切断本地补全**：按某个 API 来源过滤时，被代理记录到的其他来源仍被排除，但代理真正漏掉的本地请求会被保留（归入「未识别来源」）——以前整片本地补全会被一刀切掉
- **软删除语义**：`sync_from_scanner` 检测到会话消失时改为标记 `source_file_present = 0`，不再物理删除行；历史得以保留，同时源文件表仍能跟踪当前文件系统状态

### 修复

- **跨设备同步重复计数**：outbox 的 `request_key` 与 `event_id` 现在统一使用与本地表一致的规范化全局键（`tool:message_id` / 9 元组 fallback），修复了 WebDAV 导入后 `remote_request_facts` 出现重复行的问题
- **Schema v5 升级崩溃**：旧版本数据库（v4）启动时会因 `no such column: deleted_at in CREATE INDEX ...` 报错——v5 索引曾被错误地放在建表语句批次里、早于迁移补列时机；现已只在 v5 迁移分支内创建
- **`/clear` 上下文清空导致历史下降**：用户在 Claude 内清空上下文时，之前见过的 `message_id` 现在改为软标记而非删除，历史统计不会再意外下降

---

### Added

- **Global Outbound Network Proxy**: Added app-wide outbound proxy settings for UsageMeter's own HTTP traffic, with support for HTTP / HTTPS / SOCKS5 proxies and optional authentication
- **Proxy Connectivity Diagnostics**: Added built-in connectivity tests for GitHub, Anthropic, and OpenAI targets so proxy reachability can be verified before use

### Changed

- **Network Proxy Settings UI**: Redesigned the global network proxy settings section into a compact inline editor with clearer status feedback and per-target test results
- **Request Source Presentation**: Refined request cards and statistics summaries to distinguish local records from proxy-captured records more clearly

### Fixed

- **Proxy Config Change Notification**: Added frontend notification and automatic state refresh when external tools modify proxy takeover configuration
- **Connectivity Test Reliability**: Improved proxy connectivity test classification and result display, including better handling for timeout, connect, authentication, and HTTP error cases

---

### 新增

- **全局出站网络代理**：新增应用级全局出站代理配置，用于统一管理 UsageMeter 自身 HTTP 请求的代理行为，支持 HTTP / HTTPS / SOCKS5 以及可选认证
- **代理连通性诊断**：新增 GitHub、Anthropic、OpenAI 三个目标的内建连通性测试，可在启用前验证代理可用性

### 变更

- **网络代理设置界面**：重构全局网络代理设置区为更紧凑的内联编辑形态，补充更清晰的状态反馈与目标级测试结果展示
- **请求来源展示**：优化请求卡片与统计摘要，对本地记录和代理采集记录做更明确的来源区分

### 修复

- **代理配置变更通知**：当外部工具修改代理接管配置时，前端现在会及时提示并自动刷新相关状态
- **连通性测试可靠性**：改进代理连通性测试的错误分类与结果展示，增强对超时、连接失败、认证失败和 HTTP 错误的处理

---

## [0.5.0] - 2026-05-22

### Added

- **WebDAV Multi-device Sync**: End-to-end encrypted cross-device sync via WebDAV — PBKDF2 + AES-256-GCM encryption, batch/snapshot protocol, auto-sync with configurable interval, multi-device management (list, remove, clear imported data), and sync password rotation
- **Codex Local Log Statistics**: Native parsing of Codex rollout log files; Claude Code and Codex local statistics are now tracked in parallel without any external CLI dependency
- **Unified Usage Layer**: New merged statistics layer that aggregates local-file and proxy records into a single consistent view, with accurate attribution and deduplication across both sources
- **Overview Breakdown Panel**: New attribution ranking widget on the Overview panel — ranks usage by API source, client tool, and model across the selected time window; supports sorting by cost, requests, or tokens
- **Tool Filter**: Added client-tool filter selector (Claude Code / Codex) on the Overview and Statistics panels; proxy mode additionally shows the API source filter

### Changed

- **Removed ccusage Dependency**: Local-file mode no longer relies on the external ccusage CLI — UsageMeter now scans and parses Claude Code and Codex session JSONL files directly with an incremental in-memory cache
- **Local Usage Cache & Warm-up**: Added local-file data preheating and incremental caching to reduce repeated filesystem I/O and speed up initial loads
- **Overview Refactor**: Removed the legacy window-quota display from the overview, replaced by the new usage attribution ranking panel
- **Dark Mode & Tray Styling**: Optimized tray popup appearance and dark mode colour palette across overview attribution and card components

### Fixed

- **Source Filter Visibility**: Fixed source selector not showing in proxy mode; source filter now correctly reloads statistics on change
- **Codex Request Count**: Fixed Codex request count calculation in local-file source
- **Unified Stats Coverage**: Fixed performance stats being suppressed in local-proxy hybrid mode — proxy-collected metrics are now surfaced correctly when both sources are active

---

### 新增

- **WebDAV 多端同步**：通过 WebDAV 实现端到端加密跨设备同步 — PBKDF2 + AES-256-GCM 加密、批次/快照协议、可配置间隔的自动同步、多设备管理（列表、移除设备、清理导入数据）以及同步密码轮换
- **Codex 本地日志统计**：原生解析 Codex rollout 日志文件；Claude Code 和 Codex 本地统计并行追踪，不再依赖任何外部 CLI 工具
- **统一用量聚合层**：新增合并统计层，将本地文件与代理记录聚合为统一视图，跨来源精准归因与去重
- **概览用量归因面板**：概览页新增归因排行组件，按所选时间窗口展示 API 来源、客户端工具、模型三维用量排行；支持按费用、请求数或 Token 排序
- **工具筛选器**：概览和统计面板新增客户端工具筛选（Claude Code / Codex）；代理模式额外展示 API 来源筛选器

### 变更

- **移除 ccusage 依赖**：本地文件模式不再依赖外部 ccusage CLI——UsageMeter 现在直接扫描并解析 Claude Code 和 Codex 会话 JSONL 文件，配合增量内存缓存
- **本地用量缓存与预热**：新增本地文件数据预热与增量缓存机制，减少重复文件系统 I/O，加快初始加载速度
- **概览面板重构**：移除旧版窗口配额展示区块，替换为全新用量归因排行面板
- **深色模式与托盘样式**：优化托盘弹窗外观和深色模式配色，覆盖概览归因和卡片等组件

### 修复

- **来源筛选显示**：修复代理模式下来源选择器不显示的问题；筛选器变更时现可正确刷新统计数据
- **Codex 请求计数**：修复本地文件来源下的 Codex 请求计数统计错误
- **统一统计覆盖范围**：修复本地文件+代理混合模式下性能统计被屏蔽的问题——代理采集的性能指标在两源同时活跃时现可正确返回

---

## [0.4.0] - 2026-05-04

### Added

- **Codex Proxy Takeover**: Added full Codex proxy takeover support — config takeover, source handle save/restore, OpenAI-compatible / Responses API forwarding, usage collection, independent Claude Code & Codex toggle switches in Settings, risk confirmation for ChatGPT OAuth mode, and improved SSE chunk parsing for streaming usage
- **Multi-Currency Support**: Added exchange rate synchronization and automatic currency conversion — all cost displays now support multiple currency symbols with optimized currency list layout
- **ChatGPT Subscription Query**: Added ChatGPT subscription plan and quota query integration for tracking OpenAI-managed subscription limits
- **API Source Detection**: Added API source awareness and client tool identification to distinguish traffic from different API clients (Claude Code, Codex, etc.)
- **Batch Pricing Apply**: Added ability to bulk-apply model pricing to historical records — supports exact/fuzzy matching modes, time range & source filters, preview before apply, protects cost-locked records, batch processing (1000 records per batch)
- **Reasoning Tokens**: Added `reasoning_tokens` field extraction from OpenAI Response API `output_tokens_details`, with backward-compatible schema migration
- **Overview Panel Refactor**: Redesigned overview panel with time range quick-switch support, optimized metric cards, and compact Bento-style layout
- **Statistics Enhancements**: Refactored activity graph with improved year-view interaction, auto time-range switching on month/year toggle, and optimized statistics metric cards

### Changed

- **Provider Config Preservation**: Proxy now preserves the original Claude provider configuration during takeover — configs are stored separately, proxy routing continues working when external tools switch providers, and the active provider config is restored on stop or crash recovery
- **Window Quota Management**: Moved window quota management to a secondary settings page for cleaner UI organization
- **Shared Constants**: Extracted `WINDOW_ORDER` shared constant to eliminate three duplicate definitions across the codebase

### Fixed

- **Token Calculation**: Corrected total token formula to `total = input + cacheRead + output`, fixing discrepancies in usage statistics
- **OpenAI Usage Parsing**: Fixed multiple issues with OpenAI Chat format usage parsing, improving compatibility with various API response shapes
- **Model Pricing**: Fixed custom model price deletion, separated custom vs. synced model query logic, fixed synced model count including custom models, allowed custom price set to zero, deduplicated same-name models by last_updated, added clear-sync-data option, and fixed async loading race conditions
- **Token Generation Rate**: Fixed time-base deviation in streaming request duration calculation and corrected per-model rate statistics from simple average to weighted average
- **TTFT Calculation**: Fixed Time-to-First-Token calculation using the correct time benchmark
- **Overview Panel**: Fixed donut chart risk level misjudgment, click-switch compact format display issues, and layout optimizations
- **Statistics Coverage**: Fixed incomplete data issue with time-range filtering in statistics panel
- **Risk Warning**: Added missing risk warning notice for official API proxy takeover
- **Windows Compatibility**: Preliminary fixes for Windows platform compatibility issues

---

### 新增

- **Codex 代理接管**：新增完整的 Codex 代理接管功能 — 配置接管、来源句柄保存与恢复、OpenAI 兼容 / Responses API 请求转发与用量采集、设置页独立 Claude Code 与 Codex 接管开关、ChatGPT OAuth 模式风险确认提示、改进 SSE 分块解析与流式用量统计兼容性
- **多币种支持**：新增汇率同步与多币种自动换算 — 所有费用显示支持多货币符号，优化货币列表布局
- **ChatGPT 订阅查询**：新增 ChatGPT 订阅套餐与配额查询功能，追踪 OpenAI 管理的订阅限额
- **API 来源感知**：新增 API 来源感知与客户端工具识别，区分不同 API 客户端流量（Claude Code、Codex 等）
- **价格批量应用**：支持将模型价格批量应用到历史记录 — 精确/模糊匹配模式、时间范围和来源筛选、应用前预览、保护已锁定费用记录、分批处理（每批 1000 条）
- **推理 Token 统计**：新增 OpenAI Response API `reasoning_tokens` 字段提取，向后兼容的数据库迁移
- **概览面板重构**：重新设计概览面板，支持时间范围快速切换，优化指标卡片与紧凑 Bento 布局
- **统计面板增强**：重构活动图组件，优化年视图交互，月/年切换自动更新时间范围，优化统计指标卡片

### 变更

- **供应商配置保留**：代理接管过程中保留原始 Claude 供应商配置 — 配置独立保存，外部工具切换供应商时代理路由继续工作，停止或崩溃恢复时还原当前活动供应商配置
- **窗口配额管理**：将窗口配额管理移至设置的二级页面，优化 UI 组织
- **共享常量提取**：抽取 `WINDOW_ORDER` 共享常量消除三处重复定义

### 修复

- **Token 计算**：修正总 Token 计算公式为 `total = input + cacheRead + output`，修复用量统计偏差
- **OpenAI 用量解析**：修复 OpenAI Chat 格式 usage 解析多项问题，提升不同 API 响应格式兼容性
- **模型价格**：修复自定义模型价格删除、分离自定义与同步模型查询逻辑、修复同步模型计数包含自定义模型、允许自定义价格为零、同名模型按 last_updated 去重、新增清空同步数据、修复异步加载竞态问题
- **Token 生成速率**：修复流式请求 duration_ms 时间基准偏差、修复按模型分组速率统计从简单平均改为加权平均
- **TTFT 计算**：修复首 Token 时间计算的时间基准问题
- **概览面板**：修复圆环图风险等级误判、点击切换紧凑格式显示问题及布局优化
- **统计数据完整性**：修复时间范围筛选下统计数据不全的问题
- **风险提示**：补充官方 API 代理接管的风险提示
- **Windows 兼容性**：初步修复 Windows 平台兼容性问题

---

## [0.3.0] - 2026-04-26

### Added

- **Statistics Panel**: Added a complete usage analytics panel with month/year activity views, range selection, metric cards, trends, model breakdowns, performance stats, status codes, and insights
- **Historical Cost Ledger**: Added frozen per-request cost accounting with pricing snapshot IDs to keep historical statistics stable after pricing changes
- **Daily Aggregation Cache**: Added persistent daily and model-level summaries for faster proxy-mode activity queries
- **Statistics Commands**: Added Tauri commands for statistics summary, monthly activity, and yearly activity queries
- **Shared Formatting Utilities**: Added unified request, token, cost, duration, and rate formatting helpers

### Changed

- **Statistics UI**: Replaced the previous placeholder statistics page with compact Bento-style cards and ECharts visualizations optimized for the tray window
- **Proxy Records**: Extended proxy usage records with estimated cost, pricing snapshot, and cost lock fields
- **Session Migration**: Improved legacy unmatched record handling to avoid repeated migration attempts
- **Scrollable Content**: Improved main content container behavior for compact tray layouts

### Fixed

- **Chart Styling**: Aligned statistics line charts with the design spec by enabling visible symbols and gradient area fills
- **Compact Layout**: Removed the wide yearly activity grid overflow and kept the contribution view within the tray width
- **Statistics Coverage**: Connected performance and insights sections that were previously implemented but not rendered

---

### 新增

- **统计面板**：新增完整用量分析面板，支持月/年活跃度、时间范围选择、核心指标卡、趋势图、模型分布、性能统计、状态码与洞察
- **历史费用账本**：新增按请求冻结费用与价格快照 ID，确保价格变更后历史统计保持稳定
- **日级聚合缓存**：新增代理模式下的日级与模型级持久化汇总，加快历史活跃度查询
- **统计查询命令**：新增统计摘要、月活跃度、年活跃度 Tauri 命令
- **统一格式化工具**：新增请求数、Token、费用、耗时、速率的通用格式化方法

### 变更

- **统计界面**：将原统计占位页替换为适配托盘窗口的紧凑 Bento 风格卡片与 ECharts 图表
- **代理记录结构**：扩展代理用量记录，增加估算费用、价格快照与费用锁定字段
- **会话迁移**：优化历史未匹配记录处理，避免重复迁移尝试
- **内容滚动区域**：优化主内容容器在紧凑托盘布局下的滚动表现

### 修复

- **图表样式**：统计折线图启用实心节点与渐变面积填充，对齐设计规范
- **紧凑布局**：移除年活跃度网格的宽屏横向溢出，使贡献图保持在托盘宽度内
- **统计覆盖度**：接入此前已实现但未渲染的性能与洞察区块

---

## [0.2.2] - 2026-04-25

### Added

- **New Time Window Option**: Added "Today" window to track usage from the start of current day (00:00:00)
- **Development Tooling**: Added `npm run lint` script for pre-commit validation (TypeScript + Rust checks)
- **Session Data Architecture**: Refactored session data sources - JSONL for metadata, session_stats table for performance metrics
- **Incremental Cache**: Added incremental caching mechanism for session metadata scanning to reduce filesystem I/O
- **Data Migration**: Automatic migration of existing usage_records data to session_stats table on app startup

### Changed

- **Time Window Rename**: Renamed "1d" to "24h" to clearly indicate a rolling 24-hour window
- **Model Price Matching**: Improved matching logic - exact matching requires strict consistency, fuzzy matching supports case, prefix, and separator variations
- **Custom Price Priority**: User-defined custom model prices now take precedence over open-source database prices
- **Session List UI**: Optimized session list display with better tab switching and project query logic

### Fixed

- **Cost Calculation**: Fixed overview panel cost calculation error - now reads cost field from each time window
- **Price Matching**: Fixed model price matching logic causing incorrect cost calculations
- **Session Refresh**: Fixed issue where session list did not refresh after switching data source
- **CI Compatibility**: Unified local and CI Rust versions, resolved clippy warnings

---

### 新增

- **新增时间窗口选项**：新增 "当天" 窗口，统计今天自然日内的数据（从 00:00:00 起）
- **开发工具**：新增 `npm run lint` 脚本用于提交前验证（TypeScript + Rust 检查）
- **会话数据架构**：重构会话数据源 - JSONL 负责元信息，session_stats 表负责性能指标
- **增量缓存**：会话元数据扫描新增增量缓存机制，减少文件系统 I/O
- **数据迁移**：应用启动时自动迁移现有 usage_records 数据到 session_stats 表

### 变更

- **时间窗口重命名**：将 "1天" 重命名为 "24h"，明确表示滚动 24 小时窗口
- **模型价格匹配**：优化匹配逻辑 - 精确匹配严格一致，模糊匹配支持大小写、前缀和分隔符差异
- **自定义价格优先级**：自定义模型价格优先于开源数据库价格
- **会话列表界面**：优化会话列表展示与代理查询逻辑

### 修复

- **费用计算**：修复概览面板费用计算错误问题，改为读取每个时间窗口的 cost 字段
- **价格匹配**：修复模型价格匹配逻辑导致的费用计算错误
- **会话刷新**：修复切换数据源后会话列表未刷新的问题
- **CI 兼容性**：统一本地与 CI Rust 版本，修复 clippy 警告

---

## [0.2.1] - 2026-04-22

### Added

- **Auto-start on System Boot**:
  - Integrated tauri-plugin-autostart plugin
  - Added autostart Tauri command for enabling/disabling auto-start
  - Added autoStart field to AppSettings data model
  - Added auto-start toggle UI in Settings panel
  - Supports both macOS and Windows platforms

### Fixed

- Unified total_tokens calculation logic to include cache tokens
- Resolved total_tokens calculation discrepancy with formula: `total_tokens = input + cache_create + cache_read + output`
- `total_input_tokens` now correctly calculated as: `cache_read + cache_create + input`

---

### 新增

- **开机自动启动**：
  - 集成 tauri-plugin-autostart 插件
  - 新增 autostart Tauri 命令，支持启用/禁用开机自启动
  - AppSettings 数据模型添加 autoStart 字段
  - 设置界面添加开机自启动开关 UI
  - 支持 macOS 和 Windows 双平台

### 修复

- 统一 total_tokens 计算逻辑，包含缓存 Token
- 修复 total_tokens 计算不一致问题，计算公式：`total_tokens = input + cache_create + cache_read + output`
- `total_input_tokens` 现正确计算为：`cache_read + cache_create + input`

---

## [0.2.0] - 2025-04-18

### Added

- **Sessions Panel**: Real-time session list with status indicators, model information, and usage metrics
- **Projects Panel**: Project-based usage aggregation with visual breakdown and filtering
- **Session Detail Modal**: Detailed session information including request history, token consumption, and cost breakdown
- **Dynamic Model Pricing System**:
  - Migrated from hardcoded prices to database storage
  - API-based price synchronization with automatic updates
  - User-defined custom pricing support
  - Model price management page with search, edit, and delete operations
  - Flexible matching modes: fuzzy and exact pattern matching
- **Enhanced Settings Panel**: Integrated model pricing configuration with visual management interface

### Fixed

- Proxy mode rate panel initialization and refresh feedback issues

### Note

- Windows build is not included in this release as it has not been fully tested yet

---

### 新增

- **会话面板**：实时会话列表，支持状态指示器、模型信息和用量指标展示
- **项目面板**：基于项目的用量聚合，支持可视化分解和筛选功能
- **会话详情弹窗**：详细会话信息，包含请求历史、Token 消耗和费用明细
- **动态模型价格系统**：
  - 从硬编码价格迁移到数据库存储
  - 支持 API 自动同步更新价格
  - 用户自定义价格支持
  - 模型价格管理页面，支持搜索、编辑、删除操作
  - 灵活匹配模式：模糊匹配和精确匹配
- **增强设置面板**：集成模型价格配置，提供可视化管理界面

### 修复

- 代理模式速率面板初始化及刷新反馈问题

### 说明

- 本次发布暂不包含 Windows 版本，Windows 支持尚在测试中

---

## [0.1.0] - 2025-04-15

### Added

- Real-time usage tracking for Claude Code token and request consumption
- Multiple time window support: 5h, 1d, 7d, 30d, and current month
- Customizable quotas for each time window with visual progress indicators
- Risk level alerts with warning (70%) and critical (90%) threshold indicators
- Overview panel: summary metrics, nested donut charts showing token and request usage simultaneously
- ccusage mode: Parse usage data from ccusage CLI and local JSONL files
- Proxy mode: Local HTTP proxy to intercept and analyze Claude API requests in real-time, with orphaned state recovery from abnormal termination
- Token generation rate measurement with model-level breakdown (proxy mode)
- Time to First Token (TTFT) tracking for API response speed (proxy mode)
- Success rate monitoring with detailed status code analysis (proxy mode)
- Billing type selection: token-based, request-based, or both
- Adjustable warning and critical threshold percentages
- Configurable data refresh interval (5-300 seconds)
- Multi-language support: Simplified Chinese (zh-CN), Traditional Chinese (zh-TW), and English (en-US)
- Three theme modes: Light, Dark, and System (follows OS setting)

### Technical

- Frontend: Vue 3 + TypeScript + Vite + Tailwind CSS + ECharts + Pinia
- Backend: Tauri 2.x (Rust) with native macOS integration
- Data Sources: ccusage CLI integration + local HTTP proxy
- Cross-platform architecture with macOS priority, Windows support planned

---

### 新增

- 实时追踪 Claude Code 的 Token 和请求消耗
- 多时间窗口支持：5 小时、1 天、7 天、30 天和本月
- 为每个时间窗口设置自定义配额，配合可视化进度指示器
- 风险等级告警，支持警告（70%）和严重（90%）阈值指示器
- 概览面板：汇总指标，嵌套圆环图同时展示 Token 和请求使用情况
- ccusage 模式：从 ccusage CLI 和本地 JSONL 文件解析用量数据
- 代理模式：本地 HTTP 代理实时拦截和分析 Claude API 请求，异常终止后的孤立状态自动恢复
- Token 生成速率测量，支持模型级别细分（代理模式）
- 首 Token 生成时间（TTFT）追踪 API 响应速度（代理模式）
- 成功率监控，支持详细状态码分析（代理模式）
- 计费类型选择：Token 计费、请求计费或双计费模式
- 可调整的警告和严重阈值百分比
- 可配置的数据刷新间隔（5-300 秒）
- 多语言支持：简体中文（zh-CN）、繁体中文（zh-TW）和英文（en-US）
- 三种主题模式：明亮、暗黑和跟随系统

### 技术

- 前端：Vue 3 + TypeScript + Vite + Tailwind CSS + ECharts + Pinia
- 后端：Tauri 2.x (Rust) 配合原生 macOS 集成
- 数据源：ccusage CLI 集成 + 本地 HTTP 代理
- 跨平台架构，macOS 优先，Windows 支持计划中

---

[0.7.3]: https://github.com/smileslove/UsageMeter/releases/tag/v0.7.3
[0.7.2]: https://github.com/smileslove/UsageMeter/releases/tag/v0.7.2
[0.7.1]: https://github.com/smileslove/UsageMeter/releases/tag/v0.7.1
[0.7.0]: https://github.com/smileslove/UsageMeter/releases/tag/v0.7.0
[0.6.4]: https://github.com/smileslove/UsageMeter/releases/tag/v0.6.4
[0.6.3]: https://github.com/smileslove/UsageMeter/releases/tag/v0.6.3
[0.6.2]: https://github.com/smileslove/UsageMeter/releases/tag/v0.6.2
[0.6.1]: https://github.com/smileslove/UsageMeter/releases/tag/v0.6.1
[0.6.0]: https://github.com/smileslove/UsageMeter/releases/tag/v0.6.0
[0.5.1]: https://github.com/smileslove/UsageMeter/releases/tag/v0.5.1
[0.5.0]: https://github.com/smileslove/UsageMeter/releases/tag/v0.5.0
[0.4.0]: https://github.com/smileslove/UsageMeter/releases/tag/v0.4.0
[0.3.0]: https://github.com/smileslove/UsageMeter/releases/tag/v0.3.0
[0.2.2]: https://github.com/smileslove/UsageMeter/releases/tag/v0.2.2
[0.2.1]: https://github.com/smileslove/UsageMeter/releases/tag/v0.2.1
[0.2.0]: https://github.com/smileslove/UsageMeter/releases/tag/v0.2.0
[0.1.0]: https://github.com/smileslove/UsageMeter/releases/tag/v0.1.0
