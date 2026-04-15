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

### 🚧 Planned

- 📈 **Statistics Dashboard** - Usage trend line charts, model distribution donut charts, daily contribution heatmaps, etc.
- 💬 **Session Management** - Browse and analyze individual conversation session details, token usage, and generation rates
- 🛠️ **Multi-tool Support** - Extend support to other AI coding assistants (Cursor, Copilot, etc.)
- 🪟 **Windows Support** - Full compatibility with Windows 10/11
- ☁️ **WebDAV Sync** - Sync settings and data across devices, aggregate multi-device usage
- 📋 **Claude Pro Support** - Support usage query and monitoring for Claude Pro subscriptions with usage query APIs

---

## Screenshots

<div align="center">
  <img src="assets/overview.png" alt="Overview Panel" width="400" >
  <br>
  <em>Overview Panel</em>
</div>
> Statistics and Session panels are under development

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
| **ccusage + Local Files** | Default mode. Uses ccusage tool first (requires Node.js environment), falls back to local log parsing | Basic stats and model distribution fully supported |
| **Local Proxy** | Real-time data collection via local proxy | Supports **generation rate, status codes** and more |

> **Note**:
> - Both modes support basic token statistics and model distribution
> - Local Proxy mode provides richer real-time data (generation rate, response time, status code distribution, etc.)
> - **Cost statistics feature** is under development, will support configuring model pricing in settings

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

### Project Structure

```
UsageMeter/
├── src/                    # Vue frontend
│   ├── assets/             # Static assets
│   ├── components/         # Reusable UI components
│   ├── views/              # Panel views (Overview, Statistics, Sessions, Settings)
│   ├── stores/             # Pinia state management
│   └── i18n/               # Internationalization
├── src-tauri/              # Tauri/Rust backend
│   └── src/
│       ├── commands/       # Tauri commands
│       ├── models/         # Data models
│       ├── proxy/          # Proxy server implementation
│       └── utils/          # Utilities
├── scripts/                # Build scripts
└── assets/                 # Documentation assets
```

## Database Design

UsageMeter uses SQLite to store usage data in proxy mode. The database file is located at `~/.usagemeter/proxy_data.db`.

### Core Data Tables

#### `usage_records` - Usage Records Table

Stores detailed data for each API request:

| Field | Type | Description |
|-------|------|-------------|
| `id` | INTEGER | Primary key |
| `timestamp` | INTEGER | Request timestamp (milliseconds) |
| `message_id` | TEXT | Unique message identifier |
| `input_tokens` | INTEGER | Input token count |
| `output_tokens` | INTEGER | Output token count |
| `cache_create_tokens` | INTEGER | Cache creation token count |
| `cache_read_tokens` | INTEGER | Cache read token count |
| `model` | TEXT | Model name |
| `session_id` | TEXT | Session ID |
| `duration_ms` | INTEGER | Request duration (milliseconds) |
| `output_tokens_per_second` | REAL | Generation rate (tokens/s) |
| `ttft_ms` | INTEGER | Time to first token |
| `status_code` | INTEGER | HTTP status code |

> **Note**: `total_tokens` field is not stored because the four token types have different prices, so simply adding them is meaningless. Actual processing = `input_tokens` + `output_tokens`.

#### `daily_summary` - Daily Summary Table

Accelerates daily aggregation queries:

| Field | Type | Description |
|-------|------|-------------|
| `date` | TEXT | Date (primary key) |
| `total_tokens` | INTEGER | Total token count |
| `input_tokens` | INTEGER | Input token count |
| `output_tokens` | INTEGER | Output token count |
| `cache_create_tokens` | INTEGER | Cache creation token count |
| `cache_read_tokens` | INTEGER | Cache read token count |
| `request_count` | INTEGER | Request count |

#### `model_usage` - Model Usage Table

Statistics grouped by date and model:

| Field | Type | Description |
|-------|------|-------------|
| `date` | TEXT | Date (composite primary key) |
| `model` | TEXT | Model name (composite primary key) |
| `total_tokens` | INTEGER | Total token count |
| `input_tokens` | INTEGER | Input token count |
| `output_tokens` | INTEGER | Output token count |
| `request_count` | INTEGER | Request count |

### Configuration Storage

Application configuration is stored in JSON format at `~/.usagemeter/settings.json`, including:

- Language and timezone settings
- Refresh interval
- Warning/danger thresholds
- Billing type (token/request/both)
- Quota limits for each time window
- Proxy configuration
- Theme settings

## Tech Stack

- **Frontend**: Vue 3 + TypeScript + Tailwind CSS + ECharts
- **Backend**: Tauri 2.x (Rust)

## Contributing

Contributions and discussions are welcome! Please feel free to submit a Pull Request.

## Acknowledgments

> The following tools were referenced during implementation:

- [ryoppippi/ccusage](https://github.com/ryoppippi/ccusage) - A command-line tool for analyzing Claude Code/Codex CLI usage from local JSONL files.
- [farion1231/cc-switch](https://github.com/farion1231/cc-switch) - A cross-platform desktop all-in-one utility for Claude Code, Codex, OpenCode, OpenClaw, and Gemini command-line tools.
- [sj719045032/claude-statistics](https://github.com/sj719045032/claude-statistics) - A macOS menu bar application for monitoring Claude Code usage, session statistics, and cost details.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

---

<div align="center">
  Made with ❤️ by <a href="https://github.com/smileslove">Smileslove</a>
</div>
