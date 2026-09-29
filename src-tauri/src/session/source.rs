use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Transcript,
    ReadonlyDatabase,
    CliLog,
    Telemetry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceDescriptor {
    pub tool_id: &'static str,
    pub kind: SourceKind,
}

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

    fn try_scan(&self) -> Result<SourceSnapshot, String> {
        Ok(self.scan())
    }

    fn source_kind(&self) -> SourceKind {
        SourceKind::Transcript
    }
}

/// Extension contract for sources that may not be backed by transcript files.
///
/// Existing adapters use the default bridge to `SessionSource::scan`; a future
/// database, CLI-log, or telemetry adapter can expose its descriptor first and
/// replace the collection payload without changing the session registry API.
pub trait UsageSource: SessionSource {
    fn descriptor(&self) -> SourceDescriptor {
        SourceDescriptor {
            tool_id: self.tool_id(),
            kind: self.source_kind(),
        }
    }

    fn collect(&self) -> Result<SourceSnapshot, String> {
        self.try_scan()
    }
}

impl<T: SessionSource + ?Sized> UsageSource for T {}
