# UsageMeter

<div align="center">
  <img src="UsageMeter.svg" alt="UsageMeter Logo" width="128" height="128">
  <p><strong>A menu bar application for monitoring LLM usage</strong></p>

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
> While using domestic LLM Coding Plans, I found that some plans charge based on request counts, but there's a lack of tools that can comprehensively track LLM usage. So I developed UsageMeter — a lightweight monitoring application focused on tracking request counts, token usage, token generation rates, and quota consumption.
>
> I'm a student and currently use Claude Code for daily development. This project primarily targets Claude Code, with plans to support more Coding Plans and AI coding tools in the future.

---

## Features

### ✅ Implemented

- 📊 **Real-time Usage Monitoring** - Track Claude Code token usage and request counts in real-time
- 🎯 **Multi-window Statistics** - Support for 5h, 24h, 7d, and monthly usage statistics
- 🌐 **Proxy Mode** - Optional local proxy for more accurate real-time tracking
- 🌍 **i18n Support** - Available in English and Chinese
- ⚙️ **Flexible Quota Settings** - Configure independent limits and warning thresholds for different time windows
- 💵 **Cost Estimation** - Sync open-source model pricing data, add custom prices, and estimate usage cost by model
- 📈 **Statistics Dashboard** - Analyze requests, tokens, cost, model breakdowns, trends, status codes, and activity heatmaps
- 💬 **Session & Project Analytics** - Browse recent sessions, project-level summaries, token usage, cost, and proxy-only performance metrics
- 🚀 **Auto Start & Native Tray UX** - Launch on login, follow system theme, and run as a lightweight menu bar app

### 🚧 Planned

- 🛠️ **Multi-tool Support** - Extend support to other AI coding assistants (Cursor, Copilot, etc.)
- 🪟 **Windows Support** - Full compatibility with Windows 10/11
- ☁️ **WebDAV Sync** - Sync settings and data across devices, aggregate multi-device usage
- 📋 **Claude Pro Support** - Support usage query and monitoring for Claude Pro subscriptions with usage query APIs

---

## Screenshots

| ![Overview Panel](assets/overview.png) | ![Activity Heatmap](assets/activity-heatmap.png) | ![Time Window Statistics](assets/time-window-statistics.png) |
|:---:|:---:|:---:|
| *Overview Panel* | *Activity Heatmap* | *Time Window Statistics* |
| ![Model Usage](assets/model-usage-display.png) | ![Recent Sessions](assets/recent-sessions.png) | ![Project Statistics](assets/project-statistics.png) |
| *Model Usage* | *Recent Sessions* | *Project Statistics* |

## Installation

### Download

Download the latest release from the [Releases](https://github.com/smileslove/UsageMeter/releases) page.

### Requirements

- macOS 11.0 (Big Sur) or later
- [Claude Code](https://claude.ai/code) installed

## Usage

1. Launch UsageMeter
2. The app will appear in your menu bar
3. Click the menu bar icon to open the dashboard
4. Configure your quota limits in Settings

### Data Collection Modes

UsageMeter supports two data collection strategies:

| Mode | Description | Feature Differences |
|------|-------------|---------------------|
| **ccusage + Local Files** | Default mode. Uses ccusage first and falls back to local JSONL parsing when needed | Supports quota windows, token/request statistics, model distribution, cost estimation, sessions, and project summaries |
| **Local Proxy** | Collects request data through a local Anthropic-compatible proxy | Adds real-time performance data such as generation rate, TTFT, status codes, request duration, and proxy-side request records |

> **Note**:
> - Local-file mode remains the default and is enough for most historical token, request, session, project, and cost statistics.
> - Proxy mode enriches the same views with runtime metrics that are not available in JSONL files, such as generation rate, TTFT, response time, and status code distribution.
> - Cost estimation uses synced open-source model pricing plus user-defined custom prices. Custom prices take priority.

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
│   │   └── statistics/     # Statistics dashboard widgets
│   ├── views/              # Panel views (Overview, Statistics, Sessions, Settings)
│   ├── stores/             # Pinia state management
│   ├── utils/              # Frontend formatting utilities
│   └── i18n/               # Internationalization
├── src-tauri/              # Tauri/Rust backend
│   └── src/
│       ├── commands/       # Tauri commands
│       ├── models/         # Data models
│       ├── proxy/          # Proxy server implementation
│       ├── session/        # Local JSONL session metadata scanner
│       └── utils/          # Utilities
├── scripts/                # Build scripts
└── assets/                 # Documentation assets
```

## Database Design

UsageMeter stores proxy records, aggregated statistics, and model pricing data in SQLite at `~/.usagemeter/proxy_data.db`. Local-file mode also reads Claude Code JSONL files directly for session metadata and historical usage.

### Core Data Tables

#### `usage_records` - Usage Records Table

Stores proxy-side per-request data:

| Field | Type | Description |
|-------|------|-------------|
| `id` | INTEGER | Primary key |
| `timestamp` | INTEGER | Request timestamp (milliseconds) |
| `message_id` | TEXT | Unique message identifier |
| `input_tokens` / `output_tokens` | INTEGER | Input and output token counts |
| `cache_create_tokens` / `cache_read_tokens` | INTEGER | Cache write/read token counts |
| `model` | TEXT | Model name |
| `session_id` | TEXT | Session ID |
| `request_start_time` / `request_end_time` | INTEGER | Request timing boundaries |
| `duration_ms` / `ttft_ms` | INTEGER | Request duration and time to first token |
| `output_tokens_per_second` | REAL | Output generation rate |
| `status_code` | INTEGER | HTTP response status |
| `estimated_cost` | REAL | Cost frozen at write/backfill time |
| `pricing_snapshot_id` | TEXT | Pricing snapshot reference |
| `cost_locked` | INTEGER | Whether cost has been frozen |

> **Note**: `total_tokens` is not stored redundantly. It is computed as `input_tokens + cache_create_tokens + cache_read_tokens + output_tokens`, while cost calculations keep the four token categories separate because they may use different prices.

#### `session_stats` - Session Performance Table

Stores proxy-only session aggregates and is merged with local JSONL session metadata in the UI:

| Field | Description |
|-------|-------------|
| `session_id` | Primary key |
| `total_duration_ms`, `avg_output_tokens_per_second`, `avg_ttft_ms` | Performance metrics |
| `proxy_request_count`, `success_requests`, `error_requests` | Request counters |
| `total_input_tokens`, `total_output_tokens`, `total_cache_create_tokens`, `total_cache_read_tokens` | Proxy token totals |
| `models`, `first_request_time`, `last_request_time` | Session model and time range |
| `estimated_cost`, `last_updated` | Cost and update timestamp |

#### `daily_summary` - Daily Summary Table

Accelerates historical daily aggregation queries:

| Field | Type | Description |
|-------|------|-------------|
| `date` | TEXT | Date (primary key) |
| `total_tokens` / token category fields | INTEGER | Total and category token counts |
| `request_count` | INTEGER | Request count |
| `cost` | REAL | Total estimated cost |
| `success_*` fields | INTEGER / REAL | Successful-request token and cost aggregates |
| `model_count` | INTEGER | Number of models used that day |
| `success_requests` / `client_error_requests` / `server_error_requests` | INTEGER | Status-class counters |
| `finalized_at` | INTEGER | Finalization timestamp for historical aggregation |

#### `model_usage` - Model Usage Table

Statistics grouped by date and model:

| Field | Type | Description |
|-------|------|-------------|
| `date` | TEXT | Date (composite primary key) |
| `model` | TEXT | Model name (composite primary key) |
| `total_tokens` / token category fields | INTEGER | Total and category token counts |
| `request_count` | INTEGER | Request count |
| `cost` | REAL | Estimated cost |
| `success_requests` / `client_error_requests` / `server_error_requests` | INTEGER | Status-class counters |

#### `model_pricing` - Model Pricing Table

Caches synced open-source model prices and user-defined custom prices:

| Field | Description |
|-------|-------------|
| `model_id`, `display_name` | Model identity and display name |
| `input_price`, `output_price` | Price per million input/output tokens |
| `cache_read_price`, `cache_write_price` | Optional cache token prices |
| `source` | `api` or `custom` |
| `last_updated` | Last update timestamp |

### Configuration Storage

Application configuration is stored in JSON format at `~/.usagemeter/settings.json`, including:

- Language and timezone settings
- Refresh interval
- Warning/danger thresholds
- Billing type (token/request/both)
- Quota limits for `5h`, `24h`, `today`, `7d`, `30d`, and `current_month`
- Summary display window and data source (`ccusage` or `proxy`)
- Proxy port, auto-start behavior, and whether error requests are counted
- Theme settings and login auto-start
- Model pricing match mode, last sync time, and custom pricing overrides

## Tech Stack

- **Frontend**: Vue 3 + TypeScript + Vite + Tailwind CSS + Pinia + ECharts / vue-echarts
- **Backend**: Tauri 2.x (Rust) with tray icon, autostart, local proxy, and native window controls
- **Data**: ccusage, local Claude Code JSONL parsing, SQLite (`rusqlite`), and synced/custom model pricing
- **Proxy Runtime**: Tokio + Hyper + Reqwest for local Anthropic-compatible request forwarding

## Contributing

Contributions and discussions are welcome! Please feel free to submit a Pull Request.

## Acknowledgments

> The following tools were referenced during implementation:

- [ryoppippi/ccusage](https://github.com/ryoppippi/ccusage) - A command-line tool for analyzing Claude Code/Codex CLI usage from local JSONL files.
- [farion1231/cc-switch](https://github.com/farion1231/cc-switch) - A cross-platform desktop all-in-one utility for Claude Code, Codex, OpenCode, OpenClaw, and Gemini command-line tools.
- [sj719045032/claude-statistics](https://github.com/sj719045032/claude-statistics) - A macOS menu bar application for monitoring Claude Code usage, session statistics, and cost details.
- [anomalyco/models.dev](https://github.com/anomalyco/models.dev) - An open-source database of AI models.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

---

<div align="center">
  Made with ❤️ by <a href="https://github.com/smileslove">Smileslove</a>
</div>
