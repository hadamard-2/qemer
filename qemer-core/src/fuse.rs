//! Rank fusion for hybrid retrieval.
//!
//! Two retrievers produce two independently ranked lists. BM25 scores are
//! unbounded and corpus-dependent; vector distances are bounded and have the
//! opposite polarity. There is no principled constant that combines them, so
//! fusion here uses ranks only and never the scores themselves.

use std::collections::HashMap;

/// The conventional reciprocal-rank-fusion damping constant.
pub const RRF_K: f32 = 60.0;

/// Collapse a rank-ordered list of row-level snippet ids into a rank-ordered
/// list of distinct ids, keeping each id's best-ranked appearance.
///
/// Because the input is already sorted best-first, keeping the first
/// occurrence *is* keeping the maximum score, without materialising a score.
pub fn collapse(ordered: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    ordered
        .iter()
        .filter(|id| seen.insert((*id).clone()))
        .cloned()
        .collect()
}

/// Fuse rankings by reciprocal rank. Each list contributes `1 / (k + rank)`
/// per id, with `rank` 1-based. Output is sorted by score descending; ties
/// break on id so the ordering is stable across runs.
pub fn rrf(rankings: &[Vec<String>], k: f32) -> Vec<(String, f32)> {
    let mut scores: HashMap<&str, f32> = HashMap::new();
    for ranking in rankings {
        for (i, id) in ranking.iter().enumerate() {
            *scores.entry(id.as_str()).or_insert(0.0) += 1.0 / (k + (i + 1) as f32);
        }
    }
    let mut fused: Vec<(String, f32)> = scores
        .into_iter()
        .map(|(id, s)| (id.to_string(), s))
        .collect();
    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    fused
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapse_keeps_first_occurrence_and_order() {
        let ordered = vec!["s2".into(), "s1".into(), "s2".into(), "s3".into()];
        assert_eq!(collapse(&ordered), vec!["s2", "s1", "s3"]);
    }

    #[test]
    fn collapse_of_empty_is_empty() {
        assert!(collapse(&[]).is_empty());
    }

    #[test]
    fn rrf_ranks_a_snippet_found_by_both_above_one_found_by_either() {
        let bm25 = vec!["s1".to_string(), "s2".to_string()];
        let vector = vec!["s3".to_string(), "s1".to_string()];
        let fused = rrf(&[bm25, vector], RRF_K);
        assert_eq!(fused[0].0, "s1", "s1 appears in both lists and must win");
    }

    #[test]
    fn rrf_scores_are_descending() {
        let a = vec!["x".to_string(), "y".to_string(), "z".to_string()];
        let b = vec!["z".to_string(), "y".to_string()];
        let fused = rrf(&[a, b], RRF_K);
        for pair in fused.windows(2) {
            assert!(
                pair[0].1 >= pair[1].1,
                "fused output must be sorted by score"
            );
        }
    }

    #[test]
    fn rrf_with_one_empty_list_preserves_the_other_order() {
        let only = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let fused = rrf(&[only.clone(), vec![]], RRF_K);
        let ids: Vec<String> = fused.into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, only);
    }

    #[test]
    fn rrf_of_nothing_is_nothing() {
        assert!(rrf(&[vec![], vec![]], RRF_K).is_empty());
    }

    #[test]
    fn rrf_is_deterministic_for_tied_scores() {
        // "b" and "c" both appear once, at the same rank in different lists,
        // so their scores tie. Ties must not reorder run to run.
        let a = vec!["b".to_string()];
        let b = vec!["c".to_string()];
        let first = rrf(&[a.clone(), b.clone()], RRF_K);
        let second = rrf(&[a, b], RRF_K);
        assert_eq!(first, second);
    }
}
