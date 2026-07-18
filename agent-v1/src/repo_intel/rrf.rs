//! Reciprocal Rank Fusion over per-source ranked lists.

use std::collections::HashMap;
use std::path::PathBuf;

use super::types::{LocateCandidate, Snippet};

/// Standard RRF constant.
pub const RRF_K: f32 = 60.0;

#[derive(Debug, Clone)]
pub struct RankedHit {
    pub path: PathBuf,
    pub snippets: Vec<Snippet>,
    pub reason: String,
}

/// Fuse independently ranked source lists.
///
/// `RRF(path) = Σ 1 / (k + rank)` over sources that contain the path (1-based rank).
pub fn reciprocal_rank_fuse(
    lists: &[Vec<RankedHit>],
    limit: usize,
) -> Vec<LocateCandidate> {
    let mut scores: HashMap<PathBuf, f32> = HashMap::new();
    let mut reasons: HashMap<PathBuf, Vec<String>> = HashMap::new();
    let mut snippets: HashMap<PathBuf, Vec<Snippet>> = HashMap::new();

    for list in lists {
        for (idx, hit) in list.iter().enumerate() {
            let rank = (idx + 1) as f32;
            *scores.entry(hit.path.clone()).or_insert(0.0) += 1.0 / (RRF_K + rank);
            reasons
                .entry(hit.path.clone())
                .or_default()
                .push(format!("{}#{}", hit.reason, idx + 1));
            let snips = snippets.entry(hit.path.clone()).or_default();
            for s in &hit.snippets {
                if !snips.iter().any(|x| x.line == s.line && x.text == s.text) {
                    snips.push(s.clone());
                }
            }
        }
    }

    let mut out: Vec<LocateCandidate> = scores
        .into_iter()
        .map(|(path, score)| LocateCandidate {
            path: path.clone(),
            score,
            reasons: reasons.remove(&path).unwrap_or_default(),
            snippets: snippets.remove(&path).unwrap_or_default(),
        })
        .collect();

    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.path.cmp(&b.path))
    });
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agreement_boosts_shared_top_hit() {
        let path = |s: &str| RankedHit {
            path: PathBuf::from(s),
            snippets: vec![],
            reason: "t".into(),
        };
        let lists = vec![
            vec![path("a.ts"), path("b.ts"), path("c.ts")],
            vec![path("b.ts"), path("a.ts")],
            vec![path("a.ts"), path("c.ts")],
        ];
        let fused = reciprocal_rank_fuse(&lists, 3);
        assert_eq!(fused[0].path, PathBuf::from("a.ts"));
    }
}
