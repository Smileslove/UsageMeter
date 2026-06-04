# UsageMeter

<div align="center">
  <img src="UsageMeter.svg" alt="UsageMeter Logo" width="128" height="128">
  <p><strong>一款用于监控 AI 编程工具用量的轻量菜单栏应用</strong></p>

  <p>
    <img src="https://img.shields.io/badge/platform-macos-lightgrey" alt="Platform">
    <img src="https://img.shields.io/badge/license-MIT-green" alt="License">
  </p>

  <p>
    <a href="README.md">English</a> | <a href="README_ZH.md">中文</a>
  </p>
</div>

> 本项目基于AI工具开发，欢迎交流并参与贡献。
>
> 🎯 **为什么开发 UsageMeter？**
>
> 在日常使用 AI 编程套餐时，我发现有的服务按请求次数计费，有的更关心 Token 额度和响应质量，但很少有轻量托盘应用能把这些指标集中展示出来。于是我开发了 UsageMeter —— 一款专注于请求数、Token、费用、响应速度和额度消耗的轻量监控工具。
>
> 它最初围绕 Claude Code 设计，现在也已经支持 Codex 和 OpenCode，并通过统一的本地加代理数据链路来呈现统计结果。长期目标是在不打断日常工作的前提下，让 AI 编程工具的使用情况更可观测。

---

## 功能特性

### ✅ 已实现

- 📊 **多工具用量监控** - 通过本地历史和可选代理流量追踪 Claude Code、Codex、OpenCode 的使用情况
- 🎯 **多时间窗口统计** - 支持 5 小时、24 小时、当天、7 天、月度等多维度使用统计
- 🧩 **统一本地 + 代理聚合** - 将本地扫描与代理采集的请求合并到统一统计层，并尽量去重
- 📂 **原生本地数据读取** - 直接读取 Claude Code / Codex 的 JSONL 历史；对 OpenCode，则在本地 SQLite 结构与当前版本足够兼容时进行读取，无需依赖外部用量 CLI
- 🌐 **代理接管与运行时指标** - 可选本地代理，支持 Claude Code 接管、Codex 接管，以及受支持的 OpenCode 全局路由接管，并提供生成速率、TTFT、耗时、状态码等运行时指标
- 🌍 **国际化支持** - 支持简体中文、繁体中文和英文界面
- ⚙️ **灵活配额设置** - 为不同时间窗口配置独立的限额与警告阈值
- 💵 **费用估算** - 同步开源模型价格库、添加自定义价格，按模型估算使用费用；支持批量将价格应用到历史记录
- 📈 **统计仪表盘** - 分析请求数、Token、费用、模型分布、趋势、状态码和活跃热力图
- 💬 **会话与项目分析** - 浏览最近会话、项目汇总、Token 使用、费用和代理模式下的性能指标
- 🔀 **多工具与多来源筛选** - 按客户端工具（Claude Code / Codex）和 API 来源/供应商过滤用量
- 🏆 **用量归因排行** - 概览面板按来源、工具、模型三维展示所选时间窗口的用量排行
- 📦 **Codex 配额卡片** - 检测到 Codex 正在使用 ChatGPT OAuth 时，可在概览面板直接展示 5 小时和 7 天额度使用率
- 🌍 **应用级出站网络代理** - 为 UsageMeter 自身的 HTTP 请求配置 HTTP / HTTPS / SOCKS5 代理，并提供内建连通性测试
- 💱 **多币种支持** - 自动同步汇率，支持任意货币显示费用
- ☁️ **WebDAV 多端同步** - 通过 WebDAV 实现端到端加密（AES-256-GCM）跨设备同步，支持自动同步、设备管理和密码轮换
- 🎨 **主题调色板系统** - 支持跟随系统 / 浅色 / 深色外观，并提供多组浅色与深色调色板
- 🚀 **开机启动与原生托盘体验** - 支持登录后自动启动、跟随系统主题，并以轻量菜单栏应用运行

### 🚧 计划中

- 🛠️ **更多工具支持** - 扩展支持其他 AI 编程助手（如 Cursor、Copilot 等）
- 🪟 **Windows 支持** - 全面适配 Windows 10/11 平台
- 📋 **更广泛的订阅额度集成** - 在当前 Codex ChatGPT OAuth 配额卡片之外，继续扩展更多订阅额度查询能力

---

## 截图

|      ![概览面板](assets/overview.png)       | ![活跃度热力图](assets/activity-heatmap.png) | ![时间范围统计](assets/time-window-statistics.png) |
| :-----------------------------------------: | :------------------------------------------: | :------------------------------------------------: |
|                 _概览面板_                  |                _活跃度热力图_                |                   _时间范围统计_                   |
| ![模型调用](assets/model-usage-display.png) |   ![最近会话](assets/recent-sessions.png)    |     ![项目统计](assets/project-statistics.png)     |
|                 _模型调用_                  |                  _最近会话_                  |                     _项目统计_                     |

## 安装

### 下载

从 [Releases](https://github.com/smileslove/UsageMeter/releases) 页面下载最新版本。

### 系统要求

- macOS 11.0 (Big Sur) 或更高版本
- 若要进行本地/代理追踪，至少安装一种受支持工具：[Claude Code](https://claude.ai/code)、[Codex CLI](https://github.com/openai/codex) 或 OpenCode

## 使用方法

1. 启动 UsageMeter
2. 应用将显示在菜单栏中
3. 点击菜单栏图标打开控制面板
4. 在设置中配置您的配额限制

### 数据采集模式

UsageMeter 支持两种数据采集策略：

| 模式 | 说明 | 功能差异 |
| ------------ | ---------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| **本地数据源** | 默认模式。直接扫描 Claude Code / Codex 的本地 JSONL 历史；对 OpenCode，则以只读方式读取当前版本足够兼容的本地 SQLite 数据库 | 支持历史配额窗口、Token/请求统计、会话、项目汇总、费用估算、工具筛选和本地优先分析 |
| **本地代理** | 通过本地代理采集请求流量；启用后可接管 Claude Code、Codex，以及受支持的 OpenCode 全局路由 | 额外补充生成速率、TTFT、请求耗时、状态码、API 来源/供应商归因，以及本地历史中没有的请求时指标 |

> **提示**：
>
> - 本地数据源模式是默认模式，无需依赖外部 CLI，即可覆盖 Claude Code、Codex、OpenCode 的大部分历史 Token、请求、会话、项目和费用统计。
> - OpenCode 本地扫描为只读模式；当本地数据库结构与当前版本部分兼容时，UsageMeter 会降级为仅消息模式，但请求统计仍可用。
> - 代理模式会为同一套视图补充本地历史中没有的运行时指标，例如生成速率、TTFT、响应时间、状态码分布和来源归因。
> - UsageMeter 会尽量将本地与代理数据合并为统一视图，减少双路径采集带来的重复统计。
> - 费用估算使用同步的开源模型价格库和用户自定义价格；自定义价格优先级更高。也可将价格批量回填至历史代理记录。

### 代理接管与来源句柄

启用代理接管后，UsageMeter 会把真实上游供应商身份与本地代理地址分离存储，这样在代理停止、应用退出或崩溃恢复时，可以尽可能安全地恢复外部工具原始配置。

- **Claude Code**：写入类似 `http://127.0.0.1:18765/claude-code/source/<id>` 的代理地址，并将真实上游句柄保存到 `~/.usagemeter/proxy_source_handles.json`
- **Codex**：在接管期间临时管理 `~/.codex/config.toml` 及相关认证/供应商状态，停止接管时恢复先前来源
- **OpenCode**：管理检测到的、带显式 `baseURL` 的全局 OpenCode 配置路由；项目级配置或环境变量仍可能覆盖该路由

这种分离方式使 UsageMeter 能够观察来源/供应商使用情况，而不会永久替换原始上游配置。

### Codex OAuth 代理风险提示

Codex 代理接管支持 API Key 形式的 OpenAI 兼容供应商，也支持仍处于试验开发中的 ChatGPT OAuth 流量代理。当 UsageMeter 检测到当前 Codex 使用 ChatGPT OAuth 时，每次你手动开启 Codex 接管都会弹出风险确认。

继续使用前请注意：

- Codex OAuth 代理路径仍是试验功能，正在开发和验证中。
- ChatGPT OAuth 属于官方账号链路。UsageMeter 不建议在该模式下开启本地代理接管。
- 该链路可能随上游认证或检测机制变化而失效。
- 异常使用模式可能带来账号访问限制、警告或服务暂停等风险。
- 继续开启该功能即表示你理解并自行承担相关风险。

如需更低风险的使用方式，建议优先使用第三方供应商、API Key 或 OpenAI 兼容配置。

### 应用级网络代理

UsageMeter 还可以为自身的出站 HTTP 请求配置可选的网络代理。这个能力与上面的本地请求捕获代理不同。

- 支持 `http`、`https`、`socks5`
- 支持可选的用户名/密码认证
- 提供 GitHub、Anthropic、OpenAI 三个目标的内建连通性测试
- 会被价格同步、汇率同步、WebDAV 同步、更新检查、订阅查询等后台任务共用

## 开发

### 环境要求

- [Node.js](https://nodejs.org/) 20+
- [Rust](https://www.rust-lang.org/) 1.70+
- [pnpm](https://pnpm.io/) 或 npm

### 快速开始

```bash
# 克隆仓库
git clone https://github.com/smileslove/UsageMeter.git
cd UsageMeter
# 安装依赖
npm install
# 开发模式运行
npm run dev:tauri
# 生产构建
npm run build:tauri
```

### 提交前验证

提交前请运行 lint 脚本确保所有检查通过（与 CI 流程一致）：

```bash
npm run lint
```

该脚本会依次执行：

- TypeScript 类型检查 (`vue-tsc --noEmit`)
- Rust 格式检查 (`cargo fmt -- --check`)
- Rust clippy 静态分析 (`cargo clippy -- -D warnings`)
- Rust 编译检查 (`cargo check`)

### 项目结构

```
UsageMeter/
├── src/                    # Vue 前端
│   ├── assets/             # 静态资源
│   ├── components/         # 可复用 UI 组件
│   │   ├── overview/       # 概览归因与排行组件
│   │   └── statistics/     # 统计仪表盘组件
│   ├── views/              # 面板视图（概览、统计、会话、设置）
│   ├── stores/             # Pinia 状态管理
│   ├── utils/              # 前端格式化工具
│   └── i18n/               # 国际化
├── src-tauri/              # Tauri/Rust 后端
│   └── src/
│       ├── commands/       # Tauri 命令
│       ├── models/         # 数据模型
│       ├── proxy/          # 代理路由、接管、请求采集与来源/供应商跟踪
│       ├── session/        # Claude Code、Codex、OpenCode 的本地历史读取层
│       ├── local_usage/    # 本地用量 SQLite 存储、物化汇总与同步导出
│       ├── net/            # 共享 HTTP 客户端工厂与应用级网络代理集成
│       ├── unified_usage/  # 本地 + 代理统一聚合统计层
│       ├── subscription/   # Codex ChatGPT OAuth 配额查询支持
│       ├── sync/           # WebDAV 多端加密同步引擎
│       └── utils/          # 工具函数
├── scripts/                # 构建脚本
└── assets/                 # 文档截图等资源
```

## 数据库设计

UsageMeter 使用 SQLite 存储代理请求记录、聚合统计和模型价格数据，数据库文件位于 `~/.usagemeter/proxy_data.db`。本地历史则单独维护在 `~/.usagemeter/local_usage.db` 中，并通过 Claude Code / Codex 本地日志以及 OpenCode 本地 SQLite 数据源构建历史用量快照。

### 核心数据表

#### `usage_records` - 使用记录表

存储代理侧单次请求数据：

| 字段                                        | 类型    | 说明                        |
| ------------------------------------------- | ------- | --------------------------- |
| `id`                                        | INTEGER | 主键                        |
| `timestamp`                                 | INTEGER | 请求时间戳（毫秒）          |
| `message_id`                                | TEXT    | 消息唯一标识                |
| `input_tokens` / `output_tokens`            | INTEGER | 输入和输出 Token 数         |
| `cache_create_tokens` / `cache_read_tokens` | INTEGER | 缓存写入/读取 Token 数      |
| `model`                                     | TEXT    | 模型名称                    |
| `session_id`                                | TEXT    | 会话 ID                     |
| `request_start_time` / `request_end_time`   | INTEGER | 请求开始/结束时间           |
| `duration_ms` / `ttft_ms`                   | INTEGER | 请求耗时和首 Token 生成时间 |
| `output_tokens_per_second`                  | REAL    | 输出 Token 生成速率         |
| `status_code`                               | INTEGER | HTTP 响应状态码             |
| `estimated_cost`                            | REAL    | 写入或回填时冻结的估算费用  |
| `pricing_snapshot_id`                       | TEXT    | 价格快照引用                |
| `cost_locked`                               | INTEGER | 费用是否已冻结              |

> **注意**：数据库不冗余存储 `total_tokens`。总量按 `input_tokens + cache_create_tokens + cache_read_tokens + output_tokens` 动态计算；费用计算会保留四类 Token 的独立价格，因为缓存 Token 可能具有不同单价。

#### `session_stats` - 会话性能统计表

存储代理模式特有的会话聚合数据，并在界面中与本地 JSONL 会话元信息合并展示：

| 字段                                                                                                | 说明               |
| --------------------------------------------------------------------------------------------------- | ------------------ |
| `session_id`                                                                                        | 主键               |
| `total_duration_ms`, `avg_output_tokens_per_second`, `avg_ttft_ms`                                  | 性能指标           |
| `proxy_request_count`, `success_requests`, `error_requests`                                         | 请求计数           |
| `total_input_tokens`, `total_output_tokens`, `total_cache_create_tokens`, `total_cache_read_tokens` | 代理侧 Token 汇总  |
| `models`, `first_request_time`, `last_request_time`                                                 | 会话模型和时间范围 |
| `estimated_cost`, `last_updated`                                                                    | 费用和更新时间     |

#### `daily_summary` - 每日汇总表

用于加速历史按日聚合查询：

| 字段                                                                   | 类型           | 说明                        |
| ---------------------------------------------------------------------- | -------------- | --------------------------- |
| `date`                                                                 | TEXT           | 日期（主键）                |
| `total_tokens` / Token 分类字段                                        | INTEGER        | 总 Token 和各分类 Token 数  |
| `request_count`                                                        | INTEGER        | 请求次数                    |
| `cost`                                                                 | REAL           | 总估算费用                  |
| `success_*` 字段                                                       | INTEGER / REAL | 成功请求的 Token 和费用聚合 |
| `model_count`                                                          | INTEGER        | 当日使用模型数量            |
| `success_requests` / `client_error_requests` / `server_error_requests` | INTEGER        | 按状态类别统计的请求数      |
| `finalized_at`                                                         | INTEGER        | 历史聚合固化时间            |

#### `model_usage` - 模型使用量表

按日期和模型分组统计：

| 字段                                                                   | 类型    | 说明                       |
| ---------------------------------------------------------------------- | ------- | -------------------------- |
| `date`                                                                 | TEXT    | 日期（联合主键）           |
| `model`                                                                | TEXT    | 模型名称（联合主键）       |
| `total_tokens` / Token 分类字段                                        | INTEGER | 总 Token 和各分类 Token 数 |
| `request_count`                                                        | INTEGER | 请求次数                   |
| `cost`                                                                 | REAL    | 估算费用                   |
| `success_requests` / `client_error_requests` / `server_error_requests` | INTEGER | 按状态类别统计的请求数     |

#### `model_pricing` - 模型价格表

缓存同步的开源模型价格和用户自定义价格：

| 字段                                    | 说明                       |
| --------------------------------------- | -------------------------- |
| `model_id`, `display_name`              | 模型标识和显示名称         |
| `input_price`, `output_price`           | 每百万输入/输出 Token 单价 |
| `cache_read_price`, `cache_write_price` | 可选的缓存 Token 单价      |
| `source`                                | `api` 或 `custom`          |
| `last_updated`                          | 最后更新时间               |

### 配置存储

应用配置以 JSON 格式存储于 `~/.usagemeter/settings.json`，包含：

- 语言、时区设置
- 刷新间隔
- 警告/危险阈值
- 计费类型（token/request/both）
- `5h`、`24h`、`today`、`7d`、`30d`、`current_month` 等时间窗口配额
- 汇总展示窗口和数据源（`local` 或 `proxy`）
- 代理端口、代理自启动策略以及是否统计错误请求
- 主题外观、调色板设置和登录后自动启动
- 模型价格匹配方式、最后同步时间和自定义价格覆盖配置
- 货币设置与已同步汇率
- 客户端工具接管偏好和来源过滤设置
- WebDAV 同步配置（地址、凭证、设备 ID、同步间隔、加密密码）

本地用量数据库存储于 `~/.usagemeter/local_usage.db`（与代理数据库分离）；Claude 代理来源句柄保存于 `~/.usagemeter/proxy_source_handles.json`。

## 技术栈

- **前端**: Vue 3 + TypeScript + Vite + Tailwind CSS + Pinia + ECharts / vue-echarts
- **后端**: Tauri 2.x (Rust)，支持托盘图标、开机自启动、本地代理和原生窗口控制
- **数据**: 本地 Claude Code / Codex 历史解析、OpenCode SQLite 扫描、SQLite (`rusqlite`) 持久化、可选代理采集，以及统一合并统计事实层
- **代理运行时**: Tokio + Hyper + Reqwest，用于本地 Anthropic 兼容请求转发

## 参与贡献

欢迎交流并参与贡献！请随时提交 Pull Request。

## 致谢

> 这里在实现时，参考或使用了一些已有工具的相关实现

- [ryoppippi/ccusage](https://github.com/ryoppippi/ccusage) - 一款从本地 JSONL 文件分析 Claude Code/Codex CLI 使用情况的命令行工具。
- [farion1231/cc-switch](https://github.com/farion1231/cc-switch) - 一款面向 Claude Code、Codex、OpenCode、OpenClaw 和 Gemini 命令行工具的跨平台桌面一站式辅助工具。
- [sj719045032/claude-statistics](https://github.com/sj719045032/claude-statistics) - 一款用于监控 Claude Code 使用情况、会话统计数据及费用明细的 macOS 菜单栏应用。
- [anomalyco/models.dev](https://github.com/anomalyco/models.dev) - 一个开源的 AI 模型数据库。
- [lobehub/lobe-icons](https://github.com/lobehub/lobe-icons) - 高质量的 AI/LLM 品牌 Logo 图标库，用于界面中的模型图标展示。

## 许可证

本项目基于 MIT 许可证开源 - 详见 [LICENSE](LICENSE) 文件。

---

<div align="center">
  由 <a href="https://github.com/smileslove">Smileslove</a> 制作
</div>
