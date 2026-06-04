use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceUpdateMode {
    PerSession,
    ReplaceAll,
}

#[derive(Debug, Clone)]
pub struct SourceSnapshot {
    pub source_id: &'static str,
    pub update_mode: SourceUpdateMode,
    pub sessions: Vec<SessionFile>,
    pub scan_fingerprint: u64,
}

#[derive(Debug, Clone)]
pub struct ParsedSessionData {
    pub meta: SessionMeta,
    pub requests: Vec<LocalRequestRecord>,
}

pub trait SessionSource: Sync {
    fn tool_id(&self) -> &'static str;
    fn scan(&self) -> SourceSnapshot;
    fn parse(&self, session: &SessionFile) -> Result<ParsedSessionData, String>;
}
