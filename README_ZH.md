# UsageMeter

<div align="center">
  <img src="UsageMeter.svg" alt="UsageMeter Logo" width="128" height="128">
  <p><strong>一款用于观察 AI 编程工具用量、额度、运行时指标与费用的轻量菜单栏应用</strong></p>

  <p>
    <img src="https://img.shields.io/badge/platform-macOS-lightgrey" alt="Platform">
    <img src="https://img.shields.io/badge/stack-Vue%203%20%2B%20Tauri%202-blue" alt="Stack">
    <img src="https://img.shields.io/badge/license-MIT-green" alt="License">
  </p>

  <p>
    <a href="README.md">English</a> | <a href="README_ZH.md">中文</a>
  </p>
</div>

UsageMeter 是一款面向 AI 编程工具重度用户的本地优先托盘应用，用来集中查看请求数、Token、缓存 Token、响应质量、来源归因、额度压力、会话、项目以及费用估算。

它围绕 macOS 菜单栏工作流设计：没有 Dock 图标，面板打开即看，尽量低打扰，但能把日常 AI 编程消耗的关键指标持续暴露出来。

## 当前定位

- `macOS 优先` 的桌面应用，技术栈为 Vue 3 + TypeScript + Tauri 2
- `五面板结构`：概览、统计、会话、网关、设置
- `本地优先`，可选代理增强，并在统一统计层合并展示

## 当前支持范围

| 能力 | 当前覆盖范围 |
| --- | --- |
| 本地历史扫描 | Claude Code、Codex CLI、OpenClaw、OpenCode、Qoder CLI / IDE / IDE CN / Work / Work CN、Gemini CLI、GitHub Copilot CLI、Hermes Agent |
| 代理接管与请求采集 | Claude Code、Codex、OpenCode 全局配置路由、Reasonix 全局配置、Gemini CLI 基于环境变量的配置；为 Claude Code 与 Codex 提供 cc-switch 共存保护 |
| 本地 API 网关 | 支持 OpenAI Chat Completions、OpenAI Responses、Anthropic Messages、Gemini GenerateContent；每个配置保存一个上游凭据，并生成一个本地客户端 Key |
| 来源 / 供应商归因 | 代理流量自动识别来源，支持来源命名、合并、删除、Key 前缀备注、来源级过滤 |
| 官方或账号额度查询 | Codex ChatGPT OAuth、Claude、Gemini CLI、GitHub Copilot |
| 第三方中转额度查询 | 已配置中转来源的 profile 化额度 / 余额查询 |

## 已实现能力

### 概览面板

- 在紧凑卡片中同时展示请求数、Token、缓存 Token 和费用估算
- 支持 `5h`、`24h`、`today`、`7d`、`30d`、`current month` 六种概览窗口
- 在存在代理运行时数据时展示响应耗时与平均生成速率
- 按 `来源`、`工具`、`模型` 三个维度展示当前窗口的归因排行
- 提供额度生存卡，将官方额度、第三方余额、本地燃烧速率和近期基线结合展示

### 统计面板

- 分析请求数、Token、费用、模型使用趋势
- 提供月度 / 年度活跃视图和贡献图风格热力图
- 支持预设时间范围与精确自定义范围
- 支持按工具和 API 来源过滤统计结果
- 支持生成分享海报，可复制到剪贴板或导出 PNG

### 会话面板

- 浏览多工具最近会话
- 查看代理请求记录与运行时指标
- 查看项目级聚合统计
- 下钻到会话详情，查看 Token、费用、模型与请求性能

### API 网关

- 为公开 HTTPS 上游创建配置，支持 OpenAI Chat Completions、OpenAI Responses、Anthropic Messages 和 Gemini GenerateContent
- 使用配置专属的本地回环地址与自动生成的 `umg_...` Key 接入客户端，由 UsageMeter 注入已保存的上游凭据
- 保持响应全链路流式传输，同时记录受支持的用量、运行时指标与来源归因
- 将手动客户端路由与工具配置接管分开管理
- 每个配置使用单一上游凭据和单一本地客户端 Key，支持替换上游凭据或撤销后重新生成本地 Key
- 尽可能恢复旧版 macOS Keychain 配置；当前设备无法读取旧凭据时会明确提示手动替换

### 设置与运维

- 开关本地代理采集
- 管理受支持工具配置的接管状态与冲突恢复
- 查看本地扫描路径与 OpenCode schema 兼容状态
- 重建本地缓存，或清理孤儿本地事实
- 管理模型价格、自定义价格与历史价格回填
- 管理汇率与展示货币
- 配置应用级出站网络代理，并对 GitHub、Anthropic、OpenAI 做连通性测试
- 配置 WebDAV 端到端加密同步、设备管理与同步密码轮换
- 配置语言、刷新间隔、统计日边界、开机启动、自动检查更新
- 提供面向 Windows 场景的 WSL 被动扫描设置
- 选择国际单位（`K/M/B`）或中文单位（`万/亿`）显示数值
- 查看 cc-switch 共存状态、夺回已礼让的接管配置，并在 cc-switch 停止后安全清理残留的本地代理地址

## 数据链路

UsageMeter 当前有两条采集路径：

| 模式 | 作用 | 更适合 |
| --- | --- | --- |
| 本地扫描 | 读取受支持工具的本地历史文件或 SQLite 数据，并落到本地 SQLite 规范化缓存 | 历史用量、会话、项目、额度窗口、费用分析 |
| 本地代理 | 通过可选本地代理捕获实时请求与运行时元数据 | TTFT、请求耗时、Token 速率、状态码、来源归因 |

应用会尽量把两条链路合并到统一视图中：本地历史负责打底，代理模式负责补足运行时维度。

API 网关复用同一本地监听器，将受支持的原生协议流量接入现有运行时与来源归因链路，不转换请求协议。

## 本地存储

- 稳定用户偏好保留在精简的本地设置文件中
- 可配置实体集合与代理运行文档存储在 `app_config.db`
- 规范化本地用量、会话事实与同步状态继续存储在本地用量 SQLite 数据库中
- 网关凭据保存在 UsageMeter 本地配置中；旧版 macOS Keychain 引用会尝试迁移，无法迁移时提示手动恢复

## 截图

|      ![概览面板](assets/overview.png)       | ![活跃度热力图](assets/activity-heatmap.png) | ![时间范围统计](assets/time-window-statistics.png) |
| :-----------------------------------------: | :------------------------------------------: | :------------------------------------------------: |
|                 _概览面板_                  |                _活跃度热力图_                |                   _时间范围统计_                   |
| ![模型调用](assets/model-usage-display.png) |   ![最近会话](assets/recent-sessions.png)    |     ![项目统计](assets/project-statistics.png)     |
|                 _模型调用_                  |                  _最近会话_                  |                     _项目统计_                     |

## 安装

从 [Releases](https://github.com/smileslove/UsageMeter/releases) 页面下载最新构建版本。

### 运行要求

- macOS 11 或更高版本
- 如果希望看到真实本地或代理数据，至少安装一种已支持的 AI 编程工具

### 说明

- 当前发版体验以 macOS 菜单栏应用为核心
- 目前已经存在部分面向 Windows 的设置能力，例如 WSL 扫描，但正式体验仍然是 macOS 优先
- 某些卡片或功能只有在检测到对应工具凭据或本地数据时才会出现

## 开发

### 环境要求

- Node.js `>= 24`
- npm `>= 10`
- Rust `1.94`，并安装 `clippy`、`rustfmt`

### 本地运行

```bash
git clone https://github.com/smileslove/UsageMeter.git
cd UsageMeter
npm install
npm run dev:tauri
```

### 构建

```bash
npm run build:tauri
```

### 校验

```bash
npm run lint
```

该命令会执行：

- `vue-tsc --noEmit`
- `cargo fmt -- --check`
- `cargo clippy -- -D warnings`
- `cargo check`

## 项目结构

```text
UsageMeter/
├── src/                    # Vue 前端
│   ├── components/         # 可复用 UI 组件
│   ├── views/              # 概览 / 统计 / 会话 / 网关 / 设置
│   ├── stores/             # Pinia 状态
│   ├── i18n/               # 国际化
│   └── utils/              # 格式化与界面辅助函数
├── src-tauri/              # Tauri 后端
│   └── src/
│       ├── app_config.rs   # 可配置实体存储与运行文档
│       ├── commands/       # Tauri 命令入口
│       ├── gateway/        # 本地 API 网关领域逻辑、审计与速率限制
│       ├── session/        # 各工具本地读取器
│       ├── proxy/          # 代理采集、接管、路由
│       ├── local_usage/    # 本地用量 SQLite 缓存
│       ├── unified_usage/  # 本地 + 代理统一统计层
│       ├── subscription/   # 额度与余额查询逻辑
│       ├── sync/           # WebDAV 加密同步
│       └── net/            # 共享 HTTP 客户端与网络代理支持
├── assets/                 # README 截图等资源
└── doc/                    # 产品与架构设计文档
```

## License

MIT
