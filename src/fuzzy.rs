// Tiny subsequence fuzzy matcher for the command palette.

/// Divisor for the length penalty: each `LEN_PENALTY_DIVISOR` chars of `text`
/// costs 1 point, so a shorter command outranks a longer one on an equal-quality
/// match. Chosen so the penalty stays small relative to the per-hit rewards
/// (1–3 each) — it only breaks ties, it never overrides a better subsequence.
const LEN_PENALTY_DIVISOR: i32 = 8;

/// Score `text` against `query` (case-insensitive). Higher is better;
/// None means `query` is not a subsequence of `text`.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut qi = 0;
    let mut s = 0i32;
    let mut last: i32 = -2;
    for (ti, c) in t.iter().enumerate() {
        if qi < q.len() && *c == q[qi] {
            s += if ti as i32 == last + 1 { 3 } else { 1 }; // reward consecutive hits
            if ti == 0 {
                s += 2; // reward matching the start
            }
            last = ti as i32;
            qi += 1;
        }
    }
    if qi == q.len() {
        Some(s - (t.len() as i32) / LEN_PENALTY_DIVISOR) // mild penalty for longer strings
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_anything() {
        assert!(score("", "anything").is_some());
    }

    #[test]
    fn subsequence_matches() {
        assert!(score("sr", "split right").is_some());
        assert!(score("splrt", "split right").is_some());
    }

    #[test]
    fn non_subsequence_is_none() {
        assert!(score("xyz", "split right").is_none());
    }

    #[test]
    fn consecutive_scores_higher_than_scattered() {
        let consecutive = score("spl", "split").unwrap();
        let scattered = score("spt", "split").unwrap();
        assert!(consecutive > scattered, "{consecutive} !> {scattered}");
    }
}
