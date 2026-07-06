use std::collections::HashSet;

/// Jaccard token-overlap at/above which two texts are treated as the same fact reworded (a strict
/// superset scores lower and survives).
const NEAR_DUPLICATE_JACCARD: f32 = 0.8;

/// Whether two texts are the same fact (normalized equality or a high token-overlap reword). Crucially
/// NOT plain substring containment: a terse text that is a substring of a richer one is a strict
/// superset, not a duplicate, so the more-informative text is kept rather than dropped. Shared by the
/// distiller's intra-batch/store dedup and `recall_memory`'s cross-store dedup (ADR 0023).
pub fn is_near_duplicate(a: &str, b: &str) -> bool {
    let na = normalize(a);
    let nb = normalize(b);
    if na == nb {
        return true;
    }
    let ta: HashSet<&str> = na.split_whitespace().collect();
    let tb: HashSet<&str> = nb.split_whitespace().collect();
    if ta.is_empty() || tb.is_empty() {
        return false;
    }
    let intersection = ta.intersection(&tb).count();
    let union = ta.union(&tb).count();
    (intersection as f32 / union as f32) >= NEAR_DUPLICATE_JACCARD
}

/// Lowercase and collapse all whitespace, for order-insensitive duplicate comparison.
fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Cosine similarity between two equal-length vectors, in `[-1, 1]`. Returns `0.0` for a length mismatch
/// or a zero-magnitude vector — degenerate inputs rank as "unrelated" rather than panicking or producing
/// NaN. Pure, so the ranking math is unit-testable in isolation from any store or provider.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0_f32;
    let mut norm_a = 0.0_f32;
    let mut norm_b = 0.0_f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 { 0.0 } else { dot / denom }
}

/// Rank `candidates` (entry-id paired with its vector) against `query` by descending cosine similarity,
/// returning the ids of the top `limit` that score strictly above `floor`. The floor drops weak matches
/// (e.g. cosine ≈ 0) so a query that matches nothing semantically yields nothing here rather than the
/// most-recent embedded entries surfaced as if relevant. A pure projection over the scoring, so the
/// hybrid recall can own candidate fetching and entry hydration separately.
pub fn rank_by_similarity<'a>(
    query: &[f32],
    candidates: impl IntoIterator<Item = (&'a str, &'a [f32])>,
    limit: usize,
    floor: f32,
) -> Vec<String> {
    let mut scored: Vec<(f32, &str)> = candidates
        .into_iter()
        .map(|(id, vector)| (cosine(query, vector), id))
        .filter(|(score, _)| *score > floor)
        .collect();
    // Descending score; a stable order on ties keeps results deterministic.
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, id)| id.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_duplicate_catches_rewords_but_keeps_a_richer_superset() {
        // Case-only difference normalizes equal → duplicate.
        assert!(is_near_duplicate(
            "Always use tabs for indentation",
            "always use tabs for indentation"
        ));
        // A high-overlap reword (one extra token) → duplicate.
        assert!(is_near_duplicate(
            "always use tabs for indentation",
            "always use tabs for indentation here"
        ));
        // A terse fact that is a substring of a much richer one is a SUPERSET, not a duplicate — the
        // richer entry must be kept (the regression: plain containment dropped it).
        assert!(!is_near_duplicate(
            "use tabs",
            "always use tabs for indentation in rust source files"
        ));
    }

    #[test]
    fn identical_vectors_score_one() {
        let v = [1.0, 2.0, 3.0];
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn orthogonal_vectors_score_zero() {
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    }

    #[test]
    fn length_mismatch_and_zero_vector_score_zero() {
        assert_eq!(cosine(&[1.0, 2.0], &[1.0]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }

    #[test]
    fn ranks_nearest_first() {
        let query = [1.0, 0.0];
        let near: Vec<f32> = vec![0.9, 0.1];
        let far: Vec<f32> = vec![-1.0, 0.0];
        // A floor below -1 keeps both, so this only checks ordering.
        let ranked = rank_by_similarity(
            &query,
            [("near", near.as_slice()), ("far", far.as_slice())],
            10,
            -2.0,
        );
        assert_eq!(ranked, vec!["near", "far"]);
    }

    #[test]
    fn floor_drops_weak_matches() {
        let query = [1.0, 0.0];
        let near: Vec<f32> = vec![0.9, 0.1];
        let orthogonal: Vec<f32> = vec![0.0, 1.0]; // cosine 0 — irrelevant
        let ranked = rank_by_similarity(
            &query,
            [
                ("near", near.as_slice()),
                ("orthogonal", orthogonal.as_slice()),
            ],
            10,
            0.1,
        );
        assert_eq!(ranked, vec!["near"], "a ~0 cosine match must be dropped");
    }
}
