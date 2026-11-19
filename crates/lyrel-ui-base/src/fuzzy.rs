//! How well a row answers a query: the one fuzzy scorer every search surface
//! shares, kept free of search-UI types so any desktop crate can rank labels
//! without importing the search modal.

/// The score `text` earns against `query`, or `None` when the query's letters
/// are not a subsequence of the text. Case-insensitive; whitespace in the
/// query is ignored.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    let needle: Vec<char> = query
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| !c.is_whitespace())
        .collect();
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    if needle.len() > hay.len() {
        return None;
    }

    // Every place the first letter occurs is a place the match could begin,
    // and the best of them is the score. Matching greedily from the left
    // alone let an early stray letter decide the alignment: `worktree`
    // against `New Worktree Tab…` spent its `w` on "New" and scattered the
    // rest, scoring the title as a poor match of the very word it contains.
    let best = (0..hay.len())
        .filter(|&i| hay[i] == needle[0])
        .filter_map(|start| align(&needle, &hay, start))
        .max()?;

    let mut score = best;
    if hay == needle {
        score += 120;
    } else if hay.starts_with(&needle) {
        score += 50;
    }
    score -= (hay.len() as i32) / 6;
    Some(score)
}

/// `needle` as a subsequence of `hay` beginning at `start`, taken greedily
/// from there; `None` when the rest of it does not fit.
fn align(needle: &[char], hay: &[char], start: usize) -> Option<i32> {
    let mut qi = 0usize;
    let mut score = 0i32;
    let mut run = 0i32;
    let mut prev_hit = false;
    for (i, ch) in hay.iter().enumerate().skip(start) {
        if qi >= needle.len() {
            break;
        }
        if *ch != needle[qi] {
            prev_hit = false;
            run = 0;
            continue;
        }
        score += 1;
        let word_start = i == 0 || !hay[i - 1].is_alphanumeric();
        if word_start {
            score += 12;
        }
        if i == 0 {
            score += 10;
        }
        if prev_hit {
            run += 1;
            score += 6 + run.min(8);
        } else {
            run = 0;
        }
        prev_hit = true;
        qi += 1;
    }
    (qi == needle.len()).then_some(score)
}

#[cfg(test)]
mod tests {
    use super::fuzzy_score;

    #[test]
    fn a_whole_word_is_found_past_an_earlier_stray_letter() {
        // The `w` of "New" used to claim the match and scatter the rest.
        let whole = fuzzy_score("worktree", "New Worktree Tab…").expect("matches");
        let scattered = fuzzy_score("worktree", "New work on tree").expect("matches");
        assert!(whole > scattered, "{whole} should beat {scattered}");
        assert_eq!(whole, 88);
    }

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }

    #[test]
    fn non_subsequence_does_not_match() {
        assert_eq!(fuzzy_score("zzz", "Split Right"), None);
        assert_eq!(fuzzy_score("thgir", "Split Right"), None);
    }

    #[test]
    fn word_initials_outrank_scattered_letters() {
        let target = fuzzy_score("sr", "Split Right").expect("matches");
        let scattered = fuzzy_score("sr", "SSH: Manage Profiles…").expect("matches");
        assert!(
            target > scattered,
            "expected 'Split Right' ({target}) to outrank 'SSH: Manage Profiles…' ({scattered})"
        );
    }

    #[test]
    fn exact_and_prefix_beat_mid_string() {
        let exact = fuzzy_score("copy", "Copy").expect("matches");
        let longer = fuzzy_score("copy", "Copy Working Directory").expect("matches");
        assert!(
            exact > longer,
            "expected exact 'Copy' ({exact}) above 'Copy Working Directory' ({longer})"
        );
    }
}
