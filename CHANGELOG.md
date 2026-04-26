# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[0.3.0]: https://github.com/smileslove/UsageMeter/releases/tag/v0.3.0
[0.2.2]: https://github.com/smileslove/UsageMeter/releases/tag/v0.2.2
[0.2.1]: https://github.com/smileslove/UsageMeter/releases/tag/v0.2.1
[0.2.0]: https://github.com/smileslove/UsageMeter/releases/tag/v0.2.0
[0.1.0]: https://github.com/smileslove/UsageMeter/releases/tag/v0.1.0
