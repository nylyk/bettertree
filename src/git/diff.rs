use gix::diff::blob::{Algorithm, InternedInput, diff_with_slider_heuristics};

use super::DiffStat;

/// How much of a blob is inspected for NUL bytes before calling it binary, matching git's default.
const BINARY_PROBE: usize = 8000;

/// Counts changed lines the way `git diff --numstat` does. Binary blobs report nothing, which is
/// also what git does.
pub fn count(before: &[u8], after: &[u8]) -> DiffStat {
    if is_binary(before) || is_binary(after) {
        return DiffStat::default();
    }

    let before = String::from_utf8_lossy(before);
    let after = String::from_utf8_lossy(after);

    let input = InternedInput::new(before.as_ref(), after.as_ref());
    let diff = diff_with_slider_heuristics(Algorithm::Histogram, &input);

    DiffStat {
        added: diff.count_additions(),
        removed: diff.count_removals(),
    }
}

fn is_binary(data: &[u8]) -> bool {
    data.iter().take(BINARY_PROBE).any(|byte| *byte == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_added_and_removed_lines() {
        let stat = count(b"one\ntwo\nthree\n", b"one\nTWO\nthree\nfour\n");

        assert_eq!(stat.added, 2);
        assert_eq!(stat.removed, 1);
    }

    #[test]
    fn a_new_file_is_all_additions() {
        assert_eq!(count(b"", b"a\nb\nc\n").added, 3);
        assert_eq!(count(b"", b"a\nb\nc\n").removed, 0);
    }

    #[test]
    fn an_unchanged_file_reports_nothing() {
        assert!(count(b"same\n", b"same\n").is_empty());
    }

    #[test]
    fn binary_blobs_report_nothing() {
        assert!(count(b"a\0b", b"c\0d").is_empty());
    }
}
