use std::collections::HashSet;

/// Jaccard token-overlap at/above which two texts are treated as the same fact reworded (a strict
/// superset scores lower and survives).
const NEAR_DUPLICATE_JACCARD: f32 = 0.8;

/// Normalized equality or a high token-overlap reword — deliberately NOT substring containment: a terse
/// text inside a richer one is a superset, not a duplicate, and the richer text must survive.
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

/// Exact equality once case and whitespace normalize away — no token-overlap slack. Required wherever the
/// comparison crosses a trust boundary (ADR 0023): the model writes the project store via `remember`, so a
/// Jaccard threshold is gameable — one crafted token (a negation flip, a changed number) still clears 0.8
/// and would suppress a legitimate shared entry. The cost is missing genuine rewords, which is a
/// duplicate-listing wrinkle, not a security gap.
pub fn is_exact_normalized_duplicate(a: &str, b: &str) -> bool {
    normalize(a) == normalize(b)
}

/// Lowercase and collapse all whitespace, for order-insensitive duplicate comparison.
fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Cosine similarity in `[-1, 1]`. A length mismatch or zero-magnitude vector returns `0.0` — degenerate
/// inputs rank as "unrelated" rather than panicking or producing NaN.
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

/// The top `limit` candidate ids scoring strictly above `floor`, by descending cosine. The floor is what
/// makes a semantically-unmatched query return nothing, instead of the most-recent entries posing as hits.
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
        assert!(is_near_duplicate(
            "Always use tabs for indentation",
            "always use tabs for indentation"
        ));
        assert!(is_near_duplicate(
            "always use tabs for indentation",
            "always use tabs for indentation here"
        ));
        // A terse fact inside a much richer one is a SUPERSET: the richer entry must survive.
        assert!(!is_near_duplicate(
            "use tabs",
            "always use tabs for indentation in rust source files"
        ));
    }

    #[test]
    fn exact_normalized_duplicate_ignores_only_case_and_whitespace() {
        assert!(is_exact_normalized_duplicate(
            "Always use tabs for indentation",
            "always   use tabs for indentation"
        ));
        // A reword `is_near_duplicate` would catch must NOT match here.
        assert!(!is_exact_normalized_duplicate(
            "always use tabs for indentation",
            "always use tabs for indentation here"
        ));
        assert!(!is_exact_normalized_duplicate(
            "use tabs for indentation",
            "never use tabs for indentation"
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
