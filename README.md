# UsageMeter

<div align="center">
  <img src="UsageMeter.svg" alt="UsageMeter Logo" width="128" height="128">
  <p><strong>A lightweight menu bar app for monitoring AI coding tool usage</strong></p>

  <p>
    <img src="https://img.shields.io/badge/platform-macos-lightgrey" alt="Platform">
    <img src="https://img.shields.io/badge/license-MIT-green" alt="License">
  </p>

  <p>
    <a href="README.md">English</a> | <a href="README_ZH.md">中文</a>
  </p>
</div>

> This project is developed with AI tools. Contributions and discussions are welcome!
>
> 🎯 **Why UsageMeter?**
>
> While using AI coding plans, I found that some plans charge by request count while others care more about token budgets and runtime quality, but there was no small tray app that could track all of that in one place. So I built UsageMeter — a lightweight monitor for requests, tokens, cost, response speed, and quota consumption.
>
> I originally built it around Claude Code, and it now also tracks Codex and OpenCode with a shared local-plus-proxy data pipeline. The long-term goal is to make AI coding tool usage observable without adding friction to daily work.

---

## Features

### ✅ Implemented

- 📊 **Multi-tool Usage Monitoring** - Track Claude Code, Codex, and OpenCode usage from local history and optional proxy traffic
- 🎯 **Multi-window Statistics** - Support for 5h, 24h, Today, 7d, and monthly usage statistics
- 🧩 **Unified Local + Proxy Aggregation** - Merge local scans and proxy-captured requests into one deduplicated statistics layer
- 📂 **Native Local Data Readers** - Read Claude Code and Codex JSONL logs directly, plus OpenCode's SQLite database when its local schema is compatible enough for this build
- 🌐 **Proxy Takeover & Runtime Metrics** - Optional local proxy with Claude Code takeover, Codex takeover, and supported OpenCode global-route takeover, plus generation rate, TTFT, duration, and status code metrics
- 🌍 **i18n Support** - Available in English and Simplified/Traditional Chinese
- ⚙️ **Flexible Quota Settings** - Configure independent limits and warning thresholds for different time windows
- 💵 **Cost Estimation** - Sync open-source model pricing data, add custom prices, and estimate usage cost by model; batch-apply prices to historical records
- 📈 **Statistics Dashboard** - Analyze requests, tokens, cost, model breakdowns, trends, status codes, and activity heatmaps
- 💬 **Session & Project Analytics** - Browse recent sessions, project-level summaries, token usage, cost, and proxy-only performance metrics
- 🔀 **Multi-tool & Multi-source Filtering** - Filter usage by client tool (Claude Code / Codex) and by API source/provider
- 🏆 **Usage Attribution Breakdown** - Overview panel ranks usage by source, tool, and model for the selected time window
- 📦 **Codex Quota Card** - When ChatGPT OAuth is detected in Codex, display five-hour and seven-day quota utilization directly in the overview
- 🌍 **App-wide Outbound Network Proxy** - Route UsageMeter's own HTTP traffic through HTTP / HTTPS / SOCKS5 proxies with built-in connectivity tests
- 💱 **Multi-currency Support** - Auto-synced exchange rates with cost display in any currency
- ☁️ **WebDAV Multi-device Sync** - End-to-end encrypted (AES-256-GCM) sync across devices via WebDAV, with auto-sync, device management, and password rotation
- 🎨 **Theme Palette System** - System/light/dark appearance with multiple curated light and dark palette families
- 🚀 **Auto Start & Native Tray UX** - Launch on login, follow system theme, and run as a lightweight menu bar app

### 🚧 Planned

- 🛠️ **More Tool Support** - Extend support to other AI coding assistants (Cursor, Copilot, etc.)
- 🪟 **Windows Support** - Full compatibility with Windows 10/11
- 📋 **Broader Subscription Integrations** - Extend quota query support beyond the current Codex ChatGPT OAuth card

---

## Screenshots

|     ![Overview Panel](assets/overview.png)     | ![Activity Heatmap](assets/activity-heatmap.png) | ![Time Window Statistics](assets/time-window-statistics.png) |
| :--------------------------------------------: | :----------------------------------------------: | :----------------------------------------------------------: |
|                _Overview Panel_                |                _Activity Heatmap_                |                   _Time Window Statistics_                   |
| ![Model Usage](assets/model-usage-display.png) |  ![Recent Sessions](assets/recent-sessions.png)  |     ![Project Statistics](assets/project-statistics.png)     |
|                 _Model Usage_                  |                _Recent Sessions_                 |                     _Project Statistics_                     |

## Installation

### Download

Download the latest release from the [Releases](https://github.com/smileslove/UsageMeter/releases) page.

### Requirements

- macOS 11.0 (Big Sur) or later
- At least one supported tool installed if you want local/proxy tracking: [Claude Code](https://claude.ai/code), [Codex CLI](https://github.com/openai/codex), or OpenCode

## Usage

1. Launch UsageMeter
2. The app will appear in your menu bar
3. Click the menu bar icon to open the dashboard
4. Configure your quota limits in Settings

### Data Collection Modes

UsageMeter supports two data collection strategies:

| Mode | Description | Feature Differences |
| --------------- | ---------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| **Local Sources** | Default mode. Scans Claude Code / Codex local JSONL history and reads OpenCode's local SQLite database in read-only mode when the schema is supported well enough for this build | Supports historical quota windows, token/request statistics, sessions, project summaries, cost estimation, tool filtering, and local-first analytics |
| **Local Proxy** | Captures request traffic through a local proxy and can take over Claude Code, Codex, and supported OpenCode global routes when enabled | Adds runtime metrics such as generation rate, TTFT, duration, status codes, API-source/provider attribution, and request-time records not present in local history |

> **Note**:
>
> - Local-source mode is the default and covers most historical token, request, session, project, and cost statistics for Claude Code, Codex, and OpenCode without relying on external CLIs.
> - OpenCode local scanning is read-only and may fall back to a message-only compatibility mode when the installed OpenCode schema is only partially compatible.
> - Proxy mode enriches the same views with runtime metrics unavailable in local history, such as generation rate, TTFT, response time, status code distribution, and provider/source attribution.
> - UsageMeter merges local and proxy data into a unified view where possible, reducing duplicate counts across the two collection paths.
> - Cost estimation uses synced open-source model pricing plus user-defined custom prices. Custom prices take priority. You can also batch-apply prices to historical proxy records.

### Proxy Takeover & Source Handles

When proxy takeover is enabled, UsageMeter preserves the upstream provider identity separately from the local proxy URL so it can restore the external tool configuration safely when the proxy stops, the app exits, or crash recovery runs.

- **Claude Code**: writes a proxy URL such as `http://127.0.0.1:18765/claude-code/source/<id>` and stores the real upstream handle in `~/.usagemeter/proxy_source_handles.json`
- **Codex**: temporarily manages `~/.codex/config.toml` and related auth/provider state while takeover is active, then restores the prior source when takeover stops
- **OpenCode**: manages detected global OpenCode config routes for providers with explicit `baseURL` entries; project-local config or environment overrides may still supersede it

This separation lets UsageMeter observe source/provider usage without permanently replacing the original upstream settings.

### Codex OAuth Proxy Risk Notice

Codex proxy takeover supports both API-key style OpenAI-compatible providers and experimental ChatGPT OAuth traffic proxying. When UsageMeter detects that Codex is currently using ChatGPT OAuth, it will show a risk confirmation every time you manually enable Codex takeover.

Please read this carefully before continuing:

- The Codex OAuth proxy path is experimental and still under active development.
- ChatGPT OAuth is the official account flow. UsageMeter does not recommend enabling local proxy takeover for this mode.
- This flow may be affected by upstream authentication or detection changes at any time.
- Unusual usage patterns may lead to account access limits, warnings, or service suspension.
- Continuing to enable this feature means you understand and accept the risks yourself.

For lower-risk usage, prefer third-party providers, API-key based providers, or OpenAI-compatible configurations when possible.

### App-wide Network Proxy

UsageMeter can also route its own outbound HTTP traffic through an optional network proxy. This is separate from the local request-capture proxy above.

- Supported schemes: `http`, `https`, and `socks5`
- Optional username/password authentication
- Built-in connectivity tests for GitHub, Anthropic, and OpenAI targets before saving the config
- Shared by background tasks such as pricing sync, exchange-rate sync, WebDAV sync, updater checks, and subscription queries

## Development

### Prerequisites

- [Node.js](https://nodejs.org/) 20+
- [Rust](https://www.rust-lang.org/) 1.70+
- [pnpm](https://pnpm.io/) or npm

### Quick Start

```bash
# Clone the repository
git clone https://github.com/smileslove/UsageMeter.git
cd UsageMeter
# Install dependencies
npm install
# Run in development mode
npm run dev:tauri
# Build for production
npm run build:tauri
```

### Pre-commit Validation

Before committing, run the lint script to ensure all checks pass (same as CI):

```bash
npm run lint
```

This script runs:

- TypeScript type checking (`vue-tsc --noEmit`)
- Rust formatting check (`cargo fmt -- --check`)
- Rust clippy linting (`cargo clippy -- -D warnings`)
- Rust compilation check (`cargo check`)

### Project Structure

```
UsageMeter/
├── src/                    # Vue frontend
│   ├── assets/             # Static assets
│   ├── components/         # Reusable UI components
│   │   ├── overview/       # Overview breakdown & attribution widgets
│   │   └── statistics/     # Statistics dashboard widgets
│   ├── views/              # Panel views (Overview, Statistics, Sessions, Settings)
│   ├── stores/             # Pinia state management
│   ├── utils/              # Frontend formatting utilities
│   └── i18n/               # Internationalization
├── src-tauri/              # Tauri/Rust backend
│   └── src/
│       ├── commands/       # Tauri commands
│       ├── models/         # Data models
│       ├── proxy/          # Proxy routing, takeover, request capture, and provider/source tracking
│       ├── session/        # Local history readers for Claude Code, Codex, and OpenCode
│       ├── local_usage/    # Local usage SQLite storage, materialized summaries, and sync export
│       ├── net/            # Shared HTTP client factory and app-wide outbound proxy integration
│       ├── unified_usage/  # Merged local + proxy statistics layer
│       ├── subscription/   # Codex ChatGPT OAuth quota query support
│       ├── sync/           # WebDAV multi-device encrypted sync engine
│       └── utils/          # Utilities
├── scripts/                # Build scripts
└── assets/                 # Documentation assets
```

## Database Design

UsageMeter stores proxy records, aggregated statistics, and model pricing data in SQLite at `~/.usagemeter/proxy_data.db`. Local history is maintained separately in `~/.usagemeter/local_usage.db`, while the collectors read Claude Code / Codex local logs and OpenCode's local SQLite data source to build historical usage snapshots.

### Core Data Tables

#### `usage_records` - Usage Records Table

Stores proxy-side per-request data:

| Field                                       | Type    | Description                              |
| ------------------------------------------- | ------- | ---------------------------------------- |
| `id`                                        | INTEGER | Primary key                              |
| `timestamp`                                 | INTEGER | Request timestamp (milliseconds)         |
| `message_id`                                | TEXT    | Unique message identifier                |
| `input_tokens` / `output_tokens`            | INTEGER | Input and output token counts            |
| `cache_create_tokens` / `cache_read_tokens` | INTEGER | Cache write/read token counts            |
| `model`                                     | TEXT    | Model name                               |
| `session_id`                                | TEXT    | Session ID                               |
| `request_start_time` / `request_end_time`   | INTEGER | Request timing boundaries                |
| `duration_ms` / `ttft_ms`                   | INTEGER | Request duration and time to first token |
| `output_tokens_per_second`                  | REAL    | Output generation rate                   |
| `status_code`                               | INTEGER | HTTP response status                     |
| `estimated_cost`                            | REAL    | Cost frozen at write/backfill time       |
| `pricing_snapshot_id`                       | TEXT    | Pricing snapshot reference               |
| `cost_locked`                               | INTEGER | Whether cost has been frozen             |

> **Note**: `total_tokens` is not stored redundantly. It is computed as `input_tokens + cache_create_tokens + cache_read_tokens + output_tokens`, while cost calculations keep the four token categories separate because they may use different prices.

#### `session_stats` - Session Performance Table

Stores proxy-only session aggregates and is merged with local JSONL session metadata in the UI:

| Field                                                                                               | Description                  |
| --------------------------------------------------------------------------------------------------- | ---------------------------- |
| `session_id`                                                                                        | Primary key                  |
| `total_duration_ms`, `avg_output_tokens_per_second`, `avg_ttft_ms`                                  | Performance metrics          |
| `proxy_request_count`, `success_requests`, `error_requests`                                         | Request counters             |
| `total_input_tokens`, `total_output_tokens`, `total_cache_create_tokens`, `total_cache_read_tokens` | Proxy token totals           |
| `models`, `first_request_time`, `last_request_time`                                                 | Session model and time range |
| `estimated_cost`, `last_updated`                                                                    | Cost and update timestamp    |

#### `daily_summary` - Daily Summary Table

Accelerates historical daily aggregation queries:

| Field                                                                  | Type           | Description                                       |
| ---------------------------------------------------------------------- | -------------- | ------------------------------------------------- |
| `date`                                                                 | TEXT           | Date (primary key)                                |
| `total_tokens` / token category fields                                 | INTEGER        | Total and category token counts                   |
| `request_count`                                                        | INTEGER        | Request count                                     |
| `cost`                                                                 | REAL           | Total estimated cost                              |
| `success_*` fields                                                     | INTEGER / REAL | Successful-request token and cost aggregates      |
| `model_count`                                                          | INTEGER        | Number of models used that day                    |
| `success_requests` / `client_error_requests` / `server_error_requests` | INTEGER        | Status-class counters                             |
| `finalized_at`                                                         | INTEGER        | Finalization timestamp for historical aggregation |

#### `model_usage` - Model Usage Table

Statistics grouped by date and model:

| Field                                                                  | Type    | Description                        |
| ---------------------------------------------------------------------- | ------- | ---------------------------------- |
| `date`                                                                 | TEXT    | Date (composite primary key)       |
| `model`                                                                | TEXT    | Model name (composite primary key) |
| `total_tokens` / token category fields                                 | INTEGER | Total and category token counts    |
| `request_count`                                                        | INTEGER | Request count                      |
| `cost`                                                                 | REAL    | Estimated cost                     |
| `success_requests` / `client_error_requests` / `server_error_requests` | INTEGER | Status-class counters              |

#### `model_pricing` - Model Pricing Table

Caches synced open-source model prices and user-defined custom prices:

| Field                                   | Description                           |
| --------------------------------------- | ------------------------------------- |
| `model_id`, `display_name`              | Model identity and display name       |
| `input_price`, `output_price`           | Price per million input/output tokens |
| `cache_read_price`, `cache_write_price` | Optional cache token prices           |
| `source`                                | `api` or `custom`                     |
| `last_updated`                          | Last update timestamp                 |

### Configuration Storage

Application configuration is stored in JSON format at `~/.usagemeter/settings.json`, including:

- Language and timezone settings
- Refresh interval
- Warning/danger thresholds
- Billing type (token/request/both)
- Quota limits for `5h`, `24h`, `today`, `7d`, `30d`, and `current_month`
- Summary display window and data source (`local` or `proxy`)
- Proxy port, auto-start behavior, and whether error requests are counted
- Theme appearance, palette settings, and login auto-start
- Model pricing match mode, last sync time, and custom pricing overrides
- Currency settings and synced exchange rates
- Client-tool takeover preferences and source-aware filters
- WebDAV sync configuration (URL, credentials, device ID, interval, encryption password)

The local usage database is stored at `~/.usagemeter/local_usage.db` (separate from the proxy database), and Claude proxy source handles are kept in `~/.usagemeter/proxy_source_handles.json`.

## Tech Stack

- **Frontend**: Vue 3 + TypeScript + Vite + Tailwind CSS + Pinia + ECharts / vue-echarts
- **Backend**: Tauri 2.x (Rust) with tray icon, autostart, local proxy, and native window controls
- **Data**: local Claude Code / Codex history parsing, OpenCode SQLite scanning, SQLite (`rusqlite`) persistence, optional proxy capture, and unified merged usage facts
- **Proxy Runtime**: Tokio + Hyper + Reqwest for local Anthropic-compatible request forwarding

## Contributing

Contributions and discussions are welcome! Please feel free to submit a Pull Request.

## Acknowledgments

> The following tools were referenced or used during implementation:

- [ryoppippi/ccusage](https://github.com/ryoppippi/ccusage) - A command-line tool for analyzing Claude Code/Codex CLI usage from local JSONL files.
- [farion1231/cc-switch](https://github.com/farion1231/cc-switch) - A cross-platform desktop all-in-one utility for Claude Code, Codex, OpenCode, OpenClaw, and Gemini command-line tools.
- [sj719045032/claude-statistics](https://github.com/sj719045032/claude-statistics) - A macOS menu bar application for monitoring Claude Code usage, session statistics, and cost details.
- [anomalyco/models.dev](https://github.com/anomalyco/models.dev) - An open-source database of AI models.
- [lobehub/lobe-icons](https://github.com/lobehub/lobe-icons) - High-quality AI/LLM brand logos used for model icons in the UI.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

---

<div align="center">
  Made with ❤️ by <a href="https://github.com/smileslove">Smileslove</a>
</div>
