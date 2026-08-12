use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{AppId, ScopeId, SourceEventId};

pub const INACTIVITY_MILLIS: i64 = 90_000;
pub const MAX_SEGMENT_MILLIS: i64 = 15 * 60_000;
pub const MAX_EXTRACTION_BATCH_CHARS: usize = 12_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentBoundary {
    AppChanged,
    ProjectChanged,
    Inactivity,
    SessionEnded,
    MaximumDuration,
    CapturePaused,
    Shutdown,
    Sleep,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentInput {
    pub source_event_id: SourceEventId,
    pub app_id: Option<AppId>,
    pub scope_id: Option<ScopeId>,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    pub privacy_filtered_text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivitySegmentDraft {
    pub app_id: Option<AppId>,
    pub scope_id: Option<ScopeId>,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    pub close_reason: SegmentBoundary,
    pub source_event_ids: Vec<SourceEventId>,
    pub extraction_texts: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentTransition {
    pub closed: Option<ActivitySegmentDraft>,
    pub admitted: bool,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum SegmentationError {
    #[error("source event interval is reversed")]
    ReversedInterval,
    #[error("source events must arrive in timestamp order")]
    OutOfOrder,
    #[error("source event text exceeds the extraction batch bound")]
    OversizedSource,
}

#[derive(Default)]
pub struct ActivitySegmenter {
    open: Option<OpenSegment>,
}

struct OpenSegment {
    app_id: Option<AppId>,
    scope_id: Option<ScopeId>,
    started_at_ms: i64,
    ended_at_ms: i64,
    source_event_ids: Vec<SourceEventId>,
    extraction_texts: Vec<String>,
    seen_text: HashSet<String>,
}

impl ActivitySegmenter {
    pub fn admit(&mut self, input: SegmentInput) -> Result<SegmentTransition, SegmentationError> {
        validate_input(&input)?;
        if self
            .open
            .as_ref()
            .is_some_and(|open| input.started_at_ms < open.ended_at_ms)
        {
            return Err(SegmentationError::OutOfOrder);
        }
        let boundary = self
            .open
            .as_ref()
            .and_then(|open| boundary_for(open, &input));
        let closed = boundary.and_then(|reason| self.take_open(reason));
        let open = self.open.get_or_insert_with(|| OpenSegment::new(&input));
        let normalized = normalize_observed_text(&input.privacy_filtered_text);
        let admitted = open.seen_text.insert(normalized.clone());
        open.ended_at_ms = open.ended_at_ms.max(input.ended_at_ms);
        if admitted {
            open.source_event_ids.push(input.source_event_id);
            open.extraction_texts.push(normalized);
        }
        Ok(SegmentTransition { closed, admitted })
    }

    pub fn close(&mut self, reason: SegmentBoundary) -> Option<ActivitySegmentDraft> {
        self.take_open(reason)
    }

    fn take_open(&mut self, reason: SegmentBoundary) -> Option<ActivitySegmentDraft> {
        self.open.take().map(|open| ActivitySegmentDraft {
            app_id: open.app_id,
            scope_id: open.scope_id,
            started_at_ms: open.started_at_ms,
            ended_at_ms: open.ended_at_ms,
            close_reason: reason,
            source_event_ids: open.source_event_ids,
            extraction_texts: open.extraction_texts,
        })
    }
}

impl OpenSegment {
    fn new(input: &SegmentInput) -> Self {
        Self {
            app_id: input.app_id.clone(),
            scope_id: input.scope_id.clone(),
            started_at_ms: input.started_at_ms,
            ended_at_ms: input.ended_at_ms,
            source_event_ids: vec![],
            extraction_texts: vec![],
            seen_text: HashSet::new(),
        }
    }
}

pub fn split_extraction_batches(
    segment: &ActivitySegmentDraft,
) -> Result<Vec<Vec<String>>, SegmentationError> {
    let mut batches = Vec::new();
    let mut current = Vec::new();
    let mut chars = 0_usize;
    for text in &segment.extraction_texts {
        let text_chars = text.chars().count();
        if text_chars > MAX_EXTRACTION_BATCH_CHARS {
            return Err(SegmentationError::OversizedSource);
        }
        if !current.is_empty() && chars.saturating_add(text_chars) > MAX_EXTRACTION_BATCH_CHARS {
            batches.push(std::mem::take(&mut current));
            chars = 0;
        }
        chars = chars.saturating_add(text_chars);
        current.push(text.clone());
    }
    if !current.is_empty() {
        batches.push(current);
    }
    Ok(batches)
}

fn validate_input(input: &SegmentInput) -> Result<(), SegmentationError> {
    if input.ended_at_ms < input.started_at_ms {
        return Err(SegmentationError::ReversedInterval);
    }
    if input.privacy_filtered_text.chars().count() > MAX_EXTRACTION_BATCH_CHARS {
        return Err(SegmentationError::OversizedSource);
    }
    Ok(())
}

fn boundary_for(open: &OpenSegment, input: &SegmentInput) -> Option<SegmentBoundary> {
    if input.app_id != open.app_id {
        return Some(SegmentBoundary::AppChanged);
    }
    if input.scope_id != open.scope_id {
        return Some(SegmentBoundary::ProjectChanged);
    }
    if input.started_at_ms.saturating_sub(open.ended_at_ms) >= INACTIVITY_MILLIS {
        return Some(SegmentBoundary::Inactivity);
    }
    if input.ended_at_ms.saturating_sub(open.started_at_ms) >= MAX_SEGMENT_MILLIS {
        return Some(SegmentBoundary::MaximumDuration);
    }
    None
}

fn normalize_observed_text(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(id: &str, app: &str, start: i64, text: &str) -> SegmentInput {
        SegmentInput {
            source_event_id: SourceEventId::new(id),
            app_id: Some(AppId::new(app)),
            scope_id: None,
            started_at_ms: start,
            ended_at_ms: start + 10,
            privacy_filtered_text: text.into(),
        }
    }

    #[test]
    fn app_changes_close_segments_and_exact_text_is_collapsed() {
        let mut segmenter = ActivitySegmenter::default();
        assert!(
            segmenter
                .admit(input("s1", "slack", 0, "User likes Svelte"))
                .unwrap()
                .admitted
        );
        assert!(
            !segmenter
                .admit(input("s2", "slack", 20, " User  likes Svelte "))
                .unwrap()
                .admitted
        );
        let transition = segmenter
            .admit(input("s3", "figma", 30, "Use compact panels"))
            .unwrap();
        let closed = transition.closed.unwrap();
        assert_eq!(closed.close_reason, SegmentBoundary::AppChanged);
        assert_eq!(closed.source_event_ids, vec![SourceEventId::new("s1")]);
    }

    #[test]
    fn extraction_batches_respect_character_budget_without_truncation() {
        let segment = ActivitySegmentDraft {
            app_id: None,
            scope_id: None,
            started_at_ms: 0,
            ended_at_ms: 1,
            close_reason: SegmentBoundary::SessionEnded,
            source_event_ids: vec![],
            extraction_texts: vec!["a".repeat(7_000), "b".repeat(6_000)],
        };
        let batches = split_extraction_batches(&segment).unwrap();
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0][0].chars().count(), 7_000);
        assert_eq!(batches[1][0].chars().count(), 6_000);
    }
}
