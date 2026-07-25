use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RetrievalCandidate {
    pub reference: String,
    pub source_ref: String,
    pub path: Option<String>,
    pub heading: Option<String>,
    pub content: String,
    pub content_hash: String,
    pub source_version: Option<String>,
    pub authority: Option<String>,
    pub canonicality: Option<String>,
    pub provenance: Option<Value>,
    pub recorded_at: Option<String>,
    pub valid_time: Option<Value>,
    pub evidence_refs: Vec<String>,
    pub why_selected: Vec<String>,
    pub lane_scores: HashMap<String, f64>,
    pub score: f64,
}

pub fn reciprocal_rank_fusion(
    lanes: Vec<(&str, Vec<RetrievalCandidate>)>,
    limit: usize,
    per_source_limit: usize,
) -> Vec<RetrievalCandidate> {
    rank_fusion(lanes, limit, per_source_limit, SelectionStyle::Diversified)
}

pub fn source_coherent_rank_fusion(
    lanes: Vec<(&str, Vec<RetrievalCandidate>)>,
    limit: usize,
    per_source_limit: usize,
) -> Vec<RetrievalCandidate> {
    rank_fusion(
        lanes,
        limit,
        per_source_limit,
        SelectionStyle::SourceCoherent,
    )
}

#[derive(Clone, Copy)]
enum SelectionStyle {
    Diversified,
    SourceCoherent,
}

fn rank_fusion(
    lanes: Vec<(&str, Vec<RetrievalCandidate>)>,
    limit: usize,
    per_source_limit: usize,
    style: SelectionStyle,
) -> Vec<RetrievalCandidate> {
    let mut candidates: HashMap<String, RetrievalCandidate> = HashMap::new();
    let mut scores: HashMap<String, f64> = HashMap::new();
    let mut reasons: HashMap<String, HashSet<String>> = HashMap::new();
    let mut lane_scores: HashMap<String, HashMap<String, f64>> = HashMap::new();
    let active_lane_count = lanes
        .iter()
        .filter(|(_, results)| !results.is_empty())
        .count();
    let per_lane_guarantee = match (style, active_lane_count) {
        (_, 0) => 0,
        (SelectionStyle::SourceCoherent, _) => 1,
        // With one lane, guarantee its leader and leave the remaining slots
        // available for source diversity.
        (SelectionStyle::Diversified, 1) => 1,
        (SelectionStyle::Diversified, _) => usize::max(1, limit / active_lane_count),
    };
    let mut lane_guarantees = HashSet::new();

    for (lane, results) in lanes {
        for candidate in results.iter().take(per_lane_guarantee) {
            lane_guarantees.insert(candidate.reference.clone());
        }
        let weight = lane_weight(lane, style);
        for (rank, candidate) in results.into_iter().enumerate() {
            let reference = candidate.reference.clone();
            let native_score = candidate.score;
            candidates.entry(reference.clone()).or_insert(candidate);
            *scores.entry(reference.clone()).or_default() += weight / (60.0 + rank as f64 + 1.0);
            reasons
                .entry(reference.clone())
                .or_default()
                .insert(format!("{}_recall", lane));
            lane_scores
                .entry(reference)
                .or_default()
                .insert(lane.to_owned(), native_score);
        }
    }

    let mut ranked: Vec<_> = candidates.into_values().collect();
    ranked.sort_by(|left, right| {
        scores
            .get(&right.reference)
            .unwrap_or(&0.0)
            .total_cmp(scores.get(&left.reference).unwrap_or(&0.0))
            .then_with(|| left.source_ref.cmp(&right.source_ref))
            .then_with(|| left.reference.cmp(&right.reference))
    });

    let ranked = match style {
        SelectionStyle::Diversified => diversify_sources(ranked),
        SelectionStyle::SourceCoherent => {
            let ranked = trim_at_relevance_gap(ranked, &scores, &lane_guarantees, limit);
            group_coherent_sources(ranked)
        }
    };

    // A hybrid query must preserve the strongest candidate from every lane;
    // otherwise a broad cross-lane coincidence can erase a precise lexical,
    // temporal, or structured hit before the agent can inspect it.
    let mut guaranteed = Vec::new();
    let mut fused_remainder = Vec::new();
    for candidate in ranked {
        if lane_guarantees.contains(&candidate.reference) {
            guaranteed.push(candidate);
        } else {
            fused_remainder.push(candidate);
        }
    }
    guaranteed.extend(fused_remainder);

    let mut selected = Vec::new();
    let mut per_source = HashMap::<String, usize>::new();
    for mut candidate in guaranteed {
        if per_source.get(&candidate.source_ref).copied().unwrap_or(0) >= per_source_limit {
            continue;
        }
        candidate.score = scores[&candidate.reference];
        candidate.lane_scores = lane_scores.remove(&candidate.reference).unwrap_or_default();
        let mut selected_reasons: Vec<_> = reasons
            .remove(&candidate.reference)
            .unwrap_or_default()
            .into_iter()
            .collect();
        selected_reasons.sort();
        candidate.why_selected = selected_reasons;
        *per_source.entry(candidate.source_ref.clone()).or_default() += 1;
        selected.push(candidate);
        if selected.len() >= limit {
            break;
        }
    }
    selected
}

fn diversify_sources(ranked: Vec<RetrievalCandidate>) -> Vec<RetrievalCandidate> {
    // General queries expose breadth first. Agents can then read a selected
    // source in full or fetch neighboring chunks.
    let mut first_from_source = Vec::new();
    let mut repeated_source = Vec::new();
    let mut seen_chunk_sources = HashSet::new();
    for candidate in ranked {
        if candidate.reference.starts_with("chunk:")
            && !seen_chunk_sources.insert(candidate.source_ref.clone())
        {
            repeated_source.push(candidate);
        } else {
            first_from_source.push(candidate);
        }
    }
    first_from_source.extend(repeated_source);
    first_from_source
}

fn trim_at_relevance_gap(
    ranked: Vec<RetrievalCandidate>,
    scores: &HashMap<String, f64>,
    guarantees: &HashSet<String>,
    limit: usize,
) -> Vec<RetrievalCandidate> {
    let minimum = usize::min(4, limit);
    let mut cutoff = usize::min(limit, ranked.len());
    for index in minimum..cutoff {
        let previous = scores
            .get(&ranked[index - 1].reference)
            .copied()
            .unwrap_or(0.0);
        let next = scores.get(&ranked[index].reference).copied().unwrap_or(0.0);
        if previous > 0.0 && next / previous < 0.62 {
            cutoff = index;
            break;
        }
    }
    ranked
        .into_iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            (index < cutoff || guarantees.contains(&candidate.reference)).then_some(candidate)
        })
        .collect()
}

fn group_coherent_sources(ranked: Vec<RetrievalCandidate>) -> Vec<RetrievalCandidate> {
    let mut source_order = Vec::new();
    let mut groups = HashMap::<String, Vec<RetrievalCandidate>>::new();
    for candidate in ranked {
        if !groups.contains_key(&candidate.source_ref) {
            source_order.push(candidate.source_ref.clone());
        }
        groups
            .entry(candidate.source_ref.clone())
            .or_default()
            .push(candidate);
    }
    let mut ordered = Vec::new();
    for source in source_order {
        if let Some(candidates) = groups.remove(&source) {
            ordered.extend(candidates);
        }
    }
    ordered
}

fn lane_weight(lane: &str, style: SelectionStyle) -> f64 {
    match (style, lane) {
        // Open should trust an exact stable reference or title before broad
        // similarity, while ordinary query retains its existing balance.
        (SelectionStyle::SourceCoherent, "exact") => 3.0,
        // A temporal match can establish currentness; semantic similarity cannot.
        (_, "temporal") => 2.25,
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(reference: &str, source: &str, score: f64) -> RetrievalCandidate {
        RetrievalCandidate {
            reference: reference.to_owned(),
            source_ref: source.to_owned(),
            path: None,
            heading: None,
            content: String::new(),
            content_hash: String::new(),
            source_version: None,
            authority: None,
            canonicality: None,
            provenance: None,
            recorded_at: None,
            valid_time: None,
            evidence_refs: vec![],
            why_selected: vec![],
            lane_scores: HashMap::new(),
            score,
        }
    }

    #[test]
    fn fusion_rewards_multi_lane_hits_and_diversifies_sources() {
        let lexical = vec![
            candidate("chunk:a", "source:1", 0.9),
            candidate("chunk:b", "source:1", 0.8),
        ];
        let semantic = vec![
            candidate("chunk:b", "source:1", 0.95),
            candidate("chunk:c", "source:2", 0.7),
        ];
        let results =
            reciprocal_rank_fusion(vec![("lexical", lexical), ("semantic", semantic)], 3, 1);
        assert_eq!(results[0].reference, "chunk:b");
        assert_eq!(results.len(), 2);
        assert!(
            results[0]
                .why_selected
                .contains(&"lexical_recall".to_owned())
        );
        assert!(
            results[0]
                .why_selected
                .contains(&"semantic_recall".to_owned())
        );
    }

    #[test]
    fn temporal_evidence_beats_a_top_lexical_semantic_coincidence() {
        let stale = candidate("chunk:stale", "source:index", 0.99);
        let current = candidate("chunk:current", "source:log", 1.0);
        let results = reciprocal_rank_fusion(
            vec![
                ("lexical", vec![stale.clone()]),
                ("semantic", vec![stale]),
                ("temporal", vec![current]),
            ],
            2,
            3,
        );
        assert_eq!(results[0].reference, "chunk:current");
        assert!(
            results[0]
                .why_selected
                .contains(&"temporal_recall".to_owned())
        );
    }

    #[test]
    fn fusion_exposes_distinct_chunk_sources_before_repeated_chunks() {
        let lexical = vec![
            candidate("chunk:a", "source:1", 1.0),
            candidate("chunk:b", "source:1", 0.9),
            candidate("chunk:c", "source:2", 0.8),
        ];
        let results = reciprocal_rank_fusion(vec![("lexical", lexical)], 2, 3);
        assert_eq!(results[0].reference, "chunk:a");
        assert_eq!(results[1].reference, "chunk:c");
    }

    #[test]
    fn fusion_preserves_each_lane_leader() {
        let lexical = vec![
            candidate("chunk:lexical", "source:lexical", 1.0),
            candidate("chunk:shared", "source:shared", 0.9),
        ];
        let semantic = vec![
            candidate("chunk:semantic", "source:semantic", 1.0),
            candidate("chunk:shared", "source:shared", 0.9),
        ];
        let results =
            reciprocal_rank_fusion(vec![("lexical", lexical), ("semantic", semantic)], 3, 3);
        let references = results
            .iter()
            .map(|candidate| candidate.reference.as_str())
            .collect::<HashSet<_>>();
        assert!(references.contains("chunk:lexical"));
        assert!(references.contains("chunk:semantic"));
        assert!(references.contains("chunk:shared"));
    }

    #[test]
    fn fusion_balances_available_slots_across_active_lanes() {
        let lexical = vec![
            candidate("chunk:l1", "source:l1", 1.0),
            candidate("chunk:l2", "source:l2", 0.9),
            candidate("chunk:l3", "source:l3", 0.8),
            candidate("chunk:l4", "source:l4", 0.7),
        ];
        let semantic = vec![
            candidate("chunk:s1", "source:s1", 1.0),
            candidate("chunk:s2", "source:s2", 0.9),
            candidate("chunk:s3", "source:s3", 0.8),
            candidate("chunk:s4", "source:s4", 0.7),
        ];
        let results =
            reciprocal_rank_fusion(vec![("lexical", lexical), ("semantic", semantic)], 6, 3);
        let references = results
            .iter()
            .map(|candidate| candidate.reference.as_str())
            .collect::<HashSet<_>>();
        for reference in [
            "chunk:l1", "chunk:l2", "chunk:l3", "chunk:s1", "chunk:s2", "chunk:s3",
        ] {
            assert!(references.contains(reference));
        }
    }

    #[test]
    fn source_coherent_fusion_keeps_related_sections_together() {
        let lexical = vec![
            candidate("chunk:a", "source:primary", 1.0),
            candidate("chunk:b", "source:primary", 0.9),
            candidate("chunk:c", "source:primary", 0.8),
            candidate("chunk:d", "source:secondary", 0.7),
        ];
        let semantic = lexical.clone();
        let results =
            source_coherent_rank_fusion(vec![("lexical", lexical), ("semantic", semantic)], 4, 3);
        assert_eq!(
            results
                .iter()
                .map(|candidate| candidate.reference.as_str())
                .collect::<Vec<_>>(),
            vec!["chunk:a", "chunk:b", "chunk:c", "chunk:d"]
        );
    }

    #[test]
    fn source_coherent_fusion_drops_a_sharply_weaker_tail() {
        let lexical = vec![
            candidate("chunk:a", "source:a", 1.0),
            candidate("chunk:b", "source:b", 0.9),
            candidate("chunk:c", "source:c", 0.8),
            candidate("chunk:d", "source:d", 0.7),
            candidate("chunk:e", "source:e", 0.6),
            candidate("chunk:f", "source:f", 0.5),
        ];
        let semantic = lexical[..4].to_vec();
        let results =
            source_coherent_rank_fusion(vec![("lexical", lexical), ("semantic", semantic)], 6, 3);
        assert_eq!(results.len(), 4);
        assert!(results.iter().all(|candidate| {
            matches!(
                candidate.reference.as_str(),
                "chunk:a" | "chunk:b" | "chunk:c" | "chunk:d"
            )
        }));
    }

    #[test]
    fn source_coherent_fusion_prioritizes_an_exact_title_or_reference() {
        let exact = vec![candidate("chunk:exact", "source:exact", 1.0)];
        let lexical = vec![candidate("chunk:broad", "source:broad", 1.0)];
        let semantic = lexical.clone();
        let results = source_coherent_rank_fusion(
            vec![
                ("exact", exact),
                ("lexical", lexical),
                ("semantic", semantic),
            ],
            2,
            3,
        );
        assert_eq!(results[0].reference, "chunk:exact");
    }
}
