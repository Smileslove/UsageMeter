//! 本地会话 source 注册与调度层

use super::claude_reader::ClaudeSource;
use super::codex_reader::CodexSource;
use super::copilot_cli_reader::CopilotCliSource;
use super::deepseek_harness_reader::DeepSeekHarnessSource;
use super::gemini_reader::GeminiSource;
use super::hermes_reader::HERMES_SOURCE;
use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use super::openclaw_reader::OpenClawSource;
use super::opencode_reader::OpenCodeSource;
use super::pi_reader::PiSource;
use super::qoder_cli_reader::QoderCliSource;
use super::qoder_ide_reader::QoderIdeSource;
use super::qoder_work_reader::QoderWorkSource;
use super::source::{ParsedSessionData, SessionSource, UsageSource};

static CLAUDE_SOURCE: ClaudeSource = ClaudeSource;
static COPILOT_CLI_SOURCE: CopilotCliSource = CopilotCliSource;
static CODEX_SOURCE: CodexSource = CodexSource;
static DEEPSEEK_HARNESS_SOURCE: DeepSeekHarnessSource = DeepSeekHarnessSource;
static OPENCLAW_SOURCE: OpenClawSource = OpenClawSource;
static OPENCODE_SOURCE: OpenCodeSource = OpenCodeSource;
static PI_SOURCE: PiSource = PiSource;
static QODER_IDE_SOURCE: QoderIdeSource =
    QoderIdeSource::new(super::constants::TOOL_QODER_IDE, "Qoder");
static QODER_IDE_CN_SOURCE: QoderIdeSource =
    QoderIdeSource::new(super::constants::TOOL_QODER_IDE_CN, "QoderCN");
static QODER_CLI_SOURCE: QoderCliSource = QoderCliSource;
static QODER_WORK_SOURCE: QoderWorkSource =
    QoderWorkSource::new(super::constants::TOOL_QODER_WORK, "QoderWork", ".qoderwork");
static QODER_WORK_CN_SOURCE: QoderWorkSource = QoderWorkSource::new(
    super::constants::TOOL_QODER_WORK_CN,
    "QoderWork CN",
    ".qoderworkcn",
);
static GEMINI_SOURCE: GeminiSource = GeminiSource;

pub fn all_sources() -> [&'static dyn SessionSource; 14] {
    [
        &CLAUDE_SOURCE,
        &COPILOT_CLI_SOURCE,
        &CODEX_SOURCE,
        &DEEPSEEK_HARNESS_SOURCE,
        &OPENCLAW_SOURCE,
        &OPENCODE_SOURCE,
        &PI_SOURCE,
        &QODER_IDE_SOURCE,
        &QODER_IDE_CN_SOURCE,
        &QODER_CLI_SOURCE,
        &QODER_WORK_SOURCE,
        &QODER_WORK_CN_SOURCE,
        &GEMINI_SOURCE,
        &HERMES_SOURCE,
    ]
}

pub fn file_backed_sources() -> [&'static dyn SessionSource; 8] {
    [
        &CLAUDE_SOURCE,
        &COPILOT_CLI_SOURCE,
        &CODEX_SOURCE,
        &DEEPSEEK_HARNESS_SOURCE,
        &OPENCLAW_SOURCE,
        &PI_SOURCE,
        &QODER_CLI_SOURCE,
        &GEMINI_SOURCE,
    ]
}

pub fn scan_file_backed_session_files() -> (Vec<SessionFile>, Vec<&'static str>) {
    let mut sessions = Vec::new();
    let mut unavailable_tools = Vec::new();
    for source in file_backed_sources() {
        match source.collect() {
            Ok(snapshot) => sessions.extend(snapshot.sessions),
            Err(error) => {
                eprintln!(
                    "[UsageMeter] Local source {} unavailable: {error}",
                    source.tool_id()
                );
                unavailable_tools.push(source.tool_id());
            }
        }
    }

    sessions.sort_by_key(|session| std::cmp::Reverse(session.last_modified));
    (sessions, unavailable_tools)
}

pub fn parse_session_file_for_storage(
    session: &SessionFile,
) -> Result<(SessionMeta, Vec<LocalRequestRecord>), String> {
    let parsed = parse_session_file(session)?;
    Ok((parsed.meta, parsed.requests))
}

pub fn parse_session_file(session: &SessionFile) -> Result<ParsedSessionData, String> {
    if crate::tool_catalog::find(&session.tool)
        .filter(|descriptor| descriptor.has_local_sessions)
        .is_none()
    {
        return Err(format!("unsupported session tool: {}", session.tool));
    }
    let Some(source) = all_sources()
        .into_iter()
        .find(|source| source.tool_id() == session.tool)
    else {
        return Err(format!("unsupported session tool: {}", session.tool));
    };
    source.parse(session)
}
