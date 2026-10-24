//! Word error rate (T-140; spec 007 FR-047, SC-004, research R23).
//!
//! The one WER computation: [`score`] over [`normalize`] of both sides. Counts are
//! integers; a rate exists only when the reference has words ([`WerCounts::rate`]),
//! and a set-level rate is the sum of edits over the sum of reference words
//! ([`WerCounts::add`] then [`WerCounts::rate`]), never a mean of per-item rates.
//! Std only (decision #186). The owner-run harness is `examples/wer.rs`.

/// Lowercase, `ё` → `е`, apostrophes (`'` and U+2019) deleted, every other char
/// that is not [`char::is_alphanumeric`] a word separator. Digits stay as written
/// (`3.5` gives `3`, `5`). Total: any input gives a (possibly empty) word list.
pub fn normalize(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    for ch in text.to_lowercase().chars() {
        match ch {
            '\'' | '\u{2019}' => {}
            'ё' => word.push('е'),
            c if c.is_alphanumeric() => word.push(c),
            _ => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Edit counts of a hypothesis against a reference, and the reference's length.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct WerCounts {
    pub substitutions: usize,
    pub deletions: usize,
    pub insertions: usize,
    pub reference_words: usize,
}

impl WerCounts {
    /// Field-wise sum, in place: the set-level counts of several items.
    pub fn add(&mut self, other: &WerCounts) {
        self.substitutions += other.substitutions;
        self.deletions += other.deletions;
        self.insertions += other.insertions;
        self.reference_words += other.reference_words;
    }

    /// `(S + D + I) / N`, which may exceed 1; `None` iff `N == 0`, so nothing
    /// divides by zero.
    pub fn rate(&self) -> Option<f64> {
        if self.reference_words == 0 {
            return None;
        }
        let edits = self.substitutions + self.deletions + self.insertions;
        Some(edits as f64 / self.reference_words as f64)
    }
}

/// One DP cell: the edit cost and its split into kinds.
#[derive(Clone, Copy)]
struct Cell {
    cost: usize,
    subs: usize,
    dels: usize,
    ins: usize,
}

impl Cell {
    fn sub(self) -> Cell {
        Cell {
            cost: self.cost + 1,
            subs: self.subs + 1,
            ..self
        }
    }

    fn del(self) -> Cell {
        Cell {
            cost: self.cost + 1,
            dels: self.dels + 1,
            ..self
        }
    }

    fn ins(self) -> Cell {
        Cell {
            cost: self.cost + 1,
            ins: self.ins + 1,
            ..self
        }
    }
}

/// Word-level Levenshtein alignment of `hypothesis` against `reference` (both
/// [`normalize`] output). Two rows, `O(len(hypothesis))` memory, no indexing. On a
/// cost tie a match or substitution is preferred over a deletion, a deletion over
/// an insertion. An empty reference gives every hypothesis word as an insertion.
pub fn score(reference: &[String], hypothesis: &[String]) -> WerCounts {
    let zero = Cell {
        cost: 0,
        subs: 0,
        dels: 0,
        ins: 0,
    };
    // Row 0: the empty reference prefix against each hypothesis prefix.
    let mut prev: Vec<Cell> = Vec::with_capacity(hypothesis.len() + 1);
    let mut left = zero;
    prev.push(left);
    for _ in hypothesis {
        left = left.ins();
        prev.push(left);
    }
    let mut first = zero;
    for r in reference {
        first = first.del();
        let mut cur: Vec<Cell> = Vec::with_capacity(prev.len());
        let mut left = first;
        cur.push(left);
        for ((h, diag), up) in hypothesis.iter().zip(prev.iter()).zip(prev.iter().skip(1)) {
            let mut best = if r == h { *diag } else { diag.sub() };
            let del = up.del();
            if del.cost < best.cost {
                best = del;
            }
            let ins = left.ins();
            if ins.cost < best.cost {
                best = ins;
            }
            cur.push(best);
            left = best;
        }
        prev = cur;
    }
    let end = prev.last().copied().unwrap_or(zero);
    WerCounts {
        substitutions: end.subs,
        deletions: end.dels,
        insertions: end.ins,
        reference_words: reference.len(),
    }
}

// T050 red tests (T-140). The implementation (T051) is written above this module by the developer.
//
// API the developer implements exactly (all `pub`, std only):
//   fn normalize(text: &str) -> Vec<String>
//   fn score(reference: &[String], hypothesis: &[String]) -> WerCounts   // applied to normalize() output
//   #[derive(Debug, Default, Clone, PartialEq)]
//   struct WerCounts { pub substitutions: usize, pub deletions: usize,
//                      pub insertions: usize, pub reference_words: usize }
//   impl WerCounts {
//       fn add(&mut self, other: &WerCounts)      // field-wise sum, in place
//       fn rate(&self) -> Option<f64>             // None iff reference_words == 0
//   }

#[cfg(test)]
mod tests {
    use super::{normalize, score, WerCounts};

    fn s(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_string()).collect()
    }

    fn counts(text_ref: &str, text_hyp: &str) -> WerCounts {
        score(&normalize(text_ref), &normalize(text_hyp))
    }

    fn c(sub: usize, del: usize, ins: usize, n: usize) -> WerCounts {
        WerCounts {
            substitutions: sub,
            deletions: del,
            insertions: ins,
            reference_words: n,
        }
    }

    #[test]
    fn normalize_lowercases_strips_punctuation_keeps_numbers_maps_yo() {
        assert_eq!(
            normalize("Hello, World! 42 Ёлка"),
            s(&["hello", "world", "42", "елка"])
        );
    }

    #[test]
    fn normalize_maps_lowercase_yo_too() {
        assert_eq!(normalize("ёж всё"), s(&["еж", "все"]));
    }

    #[test]
    fn normalize_deletes_both_apostrophes_instead_of_splitting() {
        assert_eq!(normalize("DON'T"), s(&["dont"]));
        assert_eq!(normalize("don\u{2019}t"), s(&["dont"]));
    }

    #[test]
    fn normalize_treats_quotes_dashes_ellipsis_and_hyphen_as_separators() {
        assert_eq!(normalize("a«b»c—d…e"), s(&["a", "b", "c", "d", "e"]));
        assert_eq!(normalize("twenty-five"), s(&["twenty", "five"]));
        assert_eq!(normalize("3.5"), s(&["3", "5"]));
    }

    #[test]
    fn normalize_of_blank_or_punctuation_only_is_empty() {
        assert!(normalize("").is_empty());
        assert!(normalize("  ,.! \t\n—").is_empty());
    }

    #[test]
    fn score_identical_has_no_edits() {
        let r = counts("the cat sat", "The cat, sat.");
        assert_eq!(r, c(0, 0, 0, 3));
        assert_eq!(r.rate(), Some(0.0));
    }

    #[test]
    fn score_counts_one_substitution() {
        assert_eq!(counts("the cat sat", "the dog sat"), c(1, 0, 0, 3));
    }

    #[test]
    fn score_counts_one_deletion() {
        assert_eq!(counts("the cat sat", "the sat"), c(0, 1, 0, 3));
    }

    #[test]
    fn score_counts_one_insertion() {
        assert_eq!(counts("the cat sat", "the big cat sat"), c(0, 0, 1, 3));
    }

    #[test]
    fn score_counts_mixed_edits_separately() {
        // reference: a b c d ; hypothesis: a x c d e  -> 1 sub (b->x), 1 ins (e)
        assert_eq!(counts("a b c d", "a x c d e"), c(1, 0, 1, 4));
        // reference: a b c d ; hypothesis: b x d  -> 1 del (a), 1 sub (c->x)
        assert_eq!(counts("a b c d", "b x d"), c(1, 1, 0, 4));
    }

    #[test]
    fn score_empty_hypothesis_deletes_every_reference_word() {
        let r = counts("one two three", "");
        assert_eq!(r, c(0, 3, 0, 3));
        assert_eq!(r.rate(), Some(1.0));
    }

    #[test]
    fn score_both_empty_is_zero_and_rate_is_none() {
        let r = counts("", "");
        assert_eq!(r, WerCounts::default());
        assert_eq!(r.reference_words, 0);
        assert_eq!(r.rate(), None);
    }

    #[test]
    fn score_empty_reference_counts_every_hypothesis_word_as_insertion_without_rate() {
        let r = counts("", "uh huh yes");
        assert_eq!(r, c(0, 0, 3, 0));
        assert_eq!(r.rate(), None);
    }

    #[test]
    fn rate_is_edits_over_reference_words_and_may_exceed_one() {
        let r = c(1, 1, 2, 8);
        assert_eq!(r.rate(), Some(0.5));
        let many_ins = counts("a", "a b c d");
        assert_eq!(many_ins, c(0, 0, 3, 1));
        assert_eq!(many_ins.rate(), Some(3.0));
    }

    #[test]
    fn add_sums_every_field() {
        let mut total = c(1, 2, 3, 10);
        total.add(&c(4, 5, 6, 20));
        assert_eq!(total, c(5, 7, 9, 30));
    }

    #[test]
    fn set_level_rate_is_sum_of_edits_over_sum_of_words_not_mean_of_rates() {
        // item 1: 1 edit over 1 word (rate 1.0); item 2: 1 edit over 9 words (rate 1/9).
        let a = counts("a", "x");
        let b = counts("a b c d e f g h i", "a b c d e f g h x");
        assert_eq!(a, c(1, 0, 0, 1));
        assert_eq!(b, c(1, 0, 0, 9));
        let mut total = WerCounts::default();
        total.add(&a);
        total.add(&b);
        assert_eq!(total, c(2, 0, 0, 10));
        let set = total.rate().expect("non-empty reference");
        assert!((set - 0.2).abs() < 1e-12, "set rate {set}");
        let mean = (a.rate().unwrap() + b.rate().unwrap()) / 2.0;
        assert!((set - mean).abs() > 0.1, "fixture must separate the two");
    }

    #[test]
    fn set_level_keeps_insertions_of_empty_reference_items() {
        let mut total = WerCounts::default();
        total.add(&counts("", "noise words"));
        total.add(&counts("a b", "a b"));
        assert_eq!(total, c(0, 0, 2, 2));
        assert_eq!(total.rate(), Some(1.0));
    }
}
