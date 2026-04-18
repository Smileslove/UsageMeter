# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[0.2.0]: https://github.com/smileslove/UsageMeter/releases/tag/v0.2.0
[0.1.0]: https://github.com/smileslove/UsageMeter/releases/tag/v0.1.0

