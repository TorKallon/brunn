use std::{fmt, str::FromStr};

use uuid::Uuid;

use crate::error::{ApiError, ApiResult};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordKind {
    Object,
    Claim,
    Evidence,
    Relation,
    SourceEpisode,
    Asset,
    Document,
    Chunk,
    Checkpoint,
    TemporalSpec,
    RecurrenceSpec,
    CorpusRevision,
    Stage,
    Tombstone,
    DreamJob,
}

impl RecordKind {
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Object => "object",
            Self::Claim => "claim",
            Self::Evidence => "evidence",
            Self::Relation => "relation",
            Self::SourceEpisode => "source",
            Self::Asset => "asset",
            Self::Document => "document",
            Self::Chunk => "chunk",
            Self::Checkpoint => "checkpoint",
            Self::TemporalSpec => "temporal",
            Self::RecurrenceSpec => "recurrence",
            Self::CorpusRevision => "revision",
            Self::Stage => "stage",
            Self::Tombstone => "tombstone",
            Self::DreamJob => "dream",
        }
    }

    pub fn database_kind(self) -> &'static str {
        match self {
            Self::SourceEpisode => "source_episode",
            Self::TemporalSpec => "temporal_spec",
            Self::RecurrenceSpec => "recurrence_spec",
            Self::CorpusRevision => "corpus_revision",
            Self::DreamJob => "dream_job",
            other => other.prefix(),
        }
    }
}

impl FromStr for RecordKind {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "object" => Ok(Self::Object),
            "claim" => Ok(Self::Claim),
            "evidence" => Ok(Self::Evidence),
            "relation" => Ok(Self::Relation),
            "source" | "source_episode" => Ok(Self::SourceEpisode),
            "asset" => Ok(Self::Asset),
            "document" => Ok(Self::Document),
            "chunk" => Ok(Self::Chunk),
            "checkpoint" => Ok(Self::Checkpoint),
            "temporal" | "temporal_spec" => Ok(Self::TemporalSpec),
            "recurrence" | "recurrence_spec" => Ok(Self::RecurrenceSpec),
            "revision" | "corpus_revision" => Ok(Self::CorpusRevision),
            "stage" => Ok(Self::Stage),
            "tombstone" => Ok(Self::Tombstone),
            "dream" | "dream_job" => Ok(Self::DreamJob),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecordRef {
    pub kind: RecordKind,
    pub id: Uuid,
}

impl RecordRef {
    pub fn parse(value: &str) -> ApiResult<Self> {
        let (prefix, raw_id) = value
            .split_once(':')
            .ok_or_else(|| ApiError::invalid(format!("record ref lacks a kind: {value}")))?;
        let kind = RecordKind::from_str(prefix)
            .map_err(|_| ApiError::invalid(format!("unknown record ref kind: {prefix}")))?;
        let id = Uuid::parse_str(raw_id)
            .map_err(|_| ApiError::invalid(format!("record ref has an invalid UUID: {value}")))?;
        Ok(Self { kind, id })
    }

    pub fn new(kind: RecordKind) -> Self {
        Self {
            kind,
            id: Uuid::now_v7(),
        }
    }
}

impl fmt::Display for RecordRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.kind.prefix(), self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_refs_round_trip() {
        let reference = RecordRef::new(RecordKind::Object);
        assert_eq!(RecordRef::parse(&reference.to_string()).unwrap(), reference);
    }
}
