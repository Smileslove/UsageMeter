//! 本地会话 source 注册与调度层

use super::claude_reader::ClaudeSource;
use super::codex_reader::CodexSource;
use super::copilot_cli_reader::CopilotCliSource;
use super::gemini_reader::GeminiSource;
use super::hermes_reader::HERMES_SOURCE;
use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use super::openclaw_reader::OpenClawSource;
use super::opencode_reader::OpenCodeSource;
use super::qoder_cli_reader::QoderCliSource;
use super::qoder_ide_reader::QoderIdeSource;
use super::reasonix_reader::ReasonixSource;
use super::source::{ParsedSessionData, SessionSource};

static CLAUDE_SOURCE: ClaudeSource = ClaudeSource;
static COPILOT_CLI_SOURCE: CopilotCliSource = CopilotCliSource;
static CODEX_SOURCE: CodexSource = CodexSource;
static OPENCLAW_SOURCE: OpenClawSource = OpenClawSource;
static OPENCODE_SOURCE: OpenCodeSource = OpenCodeSource;
static QODER_IDE_SOURCE: QoderIdeSource =
    QoderIdeSource::new(super::constants::TOOL_QODER_IDE, "Qoder");
static QODER_IDE_CN_SOURCE: QoderIdeSource =
    QoderIdeSource::new(super::constants::TOOL_QODER_IDE_CN, "QoderCN");
static QODER_CLI_SOURCE: QoderCliSource = QoderCliSource;
static REASONIX_SOURCE: ReasonixSource = ReasonixSource;
static GEMINI_SOURCE: GeminiSource = GeminiSource;

pub fn all_sources() -> [&'static dyn SessionSource; 11] {
    [
        &CLAUDE_SOURCE,
        &COPILOT_CLI_SOURCE,
        &CODEX_SOURCE,
        &OPENCLAW_SOURCE,
        &OPENCODE_SOURCE,
        &QODER_IDE_SOURCE,
        &QODER_IDE_CN_SOURCE,
        &QODER_CLI_SOURCE,
        &REASONIX_SOURCE,
        &GEMINI_SOURCE,
        &HERMES_SOURCE,
    ]
}

pub fn file_backed_sources() -> [&'static dyn SessionSource; 7] {
    [
        &CLAUDE_SOURCE,
        &COPILOT_CLI_SOURCE,
        &CODEX_SOURCE,
        &OPENCLAW_SOURCE,
        &QODER_CLI_SOURCE,
        &REASONIX_SOURCE,
        &GEMINI_SOURCE,
    ]
}

pub fn scan_file_backed_session_files() -> Vec<SessionFile> {
    let mut sessions = Vec::new();
    for source in file_backed_sources() {
        sessions.extend(source.scan().sessions);
    }

    sessions.sort_by_key(|session| std::cmp::Reverse(session.last_modified));
    sessions
}

pub fn parse_session_file_for_storage(
    session: &SessionFile,
) -> (SessionMeta, Vec<LocalRequestRecord>) {
    let parsed = parse_session_file(session)
        .unwrap_or_else(|err| panic!("failed to parse session {}: {err}", session.session_id));
    (parsed.meta, parsed.requests)
}

pub fn parse_session_file(session: &SessionFile) -> Result<ParsedSessionData, String> {
    let Some(source) = all_sources()
        .into_iter()
        .find(|source| source.tool_id() == session.tool)
    else {
        return Err(format!("unsupported session tool: {}", session.tool));
    };
    source.parse(session)
}
