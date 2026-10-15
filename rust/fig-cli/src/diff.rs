//! Port of src/cli/diff.zig.
//!
//! Unified diff — a CLI-only line diff between two versions of the same text,
//! rendered `diff -u` style. Used by `fig fmt --diff` to show what reformatting
//! would change instead of dumping the whole reformatted file (`--dry-run`).
//!
//! Scope is deliberately narrow: two in-memory strings, line-granularity,
//! unified-diff *output* only (no patch application, no character-level
//! intraline highlighting, no three-way merge). That's the whole job `fmt
//! --diff` needs, so it's hand-rolled rather than pulled in as a dependency.
//!
//! Algorithm: classic O(N*M) dynamic-programming LCS over lines, after
//! trimming the common prefix/suffix first. The trim matters more than the
//! O(N*M) bound in practice — `fmt` diffs are almost always a small localized
//! formatting change inside an otherwise-identical file, so the DP table only
//! ever covers the actually-changed middle, not the whole file.

/// One line of the alignment between `old` and `new`: present in both
/// (`Equal`, carrying either copy since they're byte-identical), or only on
/// one side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op<'a> {
    Equal(&'a [u8]),
    Delete(&'a [u8]),
    Insert(&'a [u8]),
}

impl Op<'_> {
    fn is_equal(&self) -> bool {
        matches!(self, Op::Equal(_))
    }
}

/// Split `text` into lines without their terminating `\n`: a trailing
/// newline does NOT produce a trailing empty line. Concretely: `"a\nb\n"` ->
/// `{"a", "b"}`; `"a\nb"` -> `{"a", "b"}`; `"a\n\n"` -> `{"a", ""}`.
fn split_lines(text: &[u8]) -> Vec<&[u8]> {
    let mut lines: Vec<&[u8]> = Vec::new();
    if text.is_empty() {
        return lines;
    }
    for line in text.split(|&c| c == b'\n') {
        lines.push(line);
    }
    // `split` always yields a trailing "" segment when `text` ends in
    // '\n' (the empty tail after the last separator) — drop it so a final
    // newline doesn't register as an extra blank line.
    if text[text.len() - 1] == b'\n' {
        lines.pop();
    }
    lines
}

/// Longest-common-subsequence edit script between `old_lines` and
/// `new_lines`, appended to `out` in order. Standard backward-DP +
/// forward-walk LCS.
fn lcs_ops<'a>(old_lines: &[&'a [u8]], new_lines: &[&'a [u8]], out: &mut Vec<Op<'a>>) {
    let n = old_lines.len();
    let m = new_lines.len();
    if n == 0 && m == 0 {
        return;
    }
    if n == 0 {
        for l in new_lines {
            out.push(Op::Insert(l));
        }
        return;
    }
    if m == 0 {
        for l in old_lines {
            out.push(Op::Delete(l));
        }
        return;
    }

    // dp[i][j] = length of the LCS of old_lines[i..] and new_lines[j..],
    // flattened into one (n+1)*(m+1) row-major slice.
    let stride = m + 1;
    let mut dp = vec![0u32; (n + 1) * stride];

    let mut i = n;
    while i > 0 {
        i -= 1;
        let mut j = m;
        while j > 0 {
            j -= 1;
            dp[i * stride + j] = if old_lines[i] == new_lines[j] {
                dp[(i + 1) * stride + (j + 1)] + 1
            } else {
                dp[(i + 1) * stride + j].max(dp[i * stride + (j + 1)])
            };
        }
    }

    // Walk forward, preferring the direction the DP table says preserves the
    // LCS; ties go to `delete` before `insert` so runs come out old-then-new,
    // matching conventional diff output.
    let mut i = 0;
    let mut j = 0;
    while i < n && j < m {
        if old_lines[i] == new_lines[j] {
            out.push(Op::Equal(old_lines[i]));
            i += 1;
            j += 1;
        } else if dp[(i + 1) * stride + j] >= dp[i * stride + (j + 1)] {
            out.push(Op::Delete(old_lines[i]));
            i += 1;
        } else {
            out.push(Op::Insert(new_lines[j]));
            j += 1;
        }
    }
    while i < n {
        out.push(Op::Delete(old_lines[i]));
        i += 1;
    }
    while j < m {
        out.push(Op::Insert(new_lines[j]));
        j += 1;
    }
}

/// Full edit script between `old` and `new`, trimming the common prefix/suffix
/// first so the DP in `lcs_ops` only ever runs over the actually-changed middle.
fn diff_ops<'a>(old_lines: &[&'a [u8]], new_lines: &[&'a [u8]]) -> Vec<Op<'a>> {
    let mut prefix = 0;
    let max_prefix = old_lines.len().min(new_lines.len());
    while prefix < max_prefix && old_lines[prefix] == new_lines[prefix] {
        prefix += 1;
    }

    let mut suffix = 0;
    let max_suffix = max_prefix - prefix;
    while suffix < max_suffix && old_lines[old_lines.len() - 1 - suffix] == new_lines[new_lines.len() - 1 - suffix] {
        suffix += 1;
    }

    let mut out: Vec<Op<'a>> = Vec::new();
    for l in &old_lines[..prefix] {
        out.push(Op::Equal(l));
    }
    lcs_ops(&old_lines[prefix..old_lines.len() - suffix], &new_lines[prefix..new_lines.len() - suffix], &mut out);
    for l in &old_lines[old_lines.len() - suffix..] {
        out.push(Op::Equal(l));
    }
    out
}

/// One `@@ -a,b +c,d @@` block: `[start, end)` into the full `ops` slice.
#[derive(Debug, Clone, Copy)]
struct Hunk {
    start: usize,
    end: usize,
}

/// Group `ops` into unified-diff hunks: each changed run padded with up to
/// `context` lines of surrounding `Equal` context, merging runs whose gap is
/// small enough that their context would overlap.
fn build_hunks(ops: &[Op<'_>], context: usize) -> Vec<Hunk> {
    let mut hunks: Vec<Hunk> = Vec::new();

    let mut i = 0;
    while i < ops.len() {
        if ops[i].is_equal() {
            i += 1;
            continue;
        }
        // `i` starts a change run. Back up to include leading context, but
        // never into the previous hunk's territory.
        let prev_end = hunks.last().map_or(0, |h| h.end);
        let mut start = i;
        let mut back = 0;
        while start > prev_end && back < context && ops[start - 1].is_equal() {
            start -= 1;
            back += 1;
        }

        // Extend forward past this change run, absorbing any further change
        // run that's within `2*context` equal lines of this one (so their
        // context regions would otherwise overlap), then stop and take
        // trailing context.
        let mut end = i;
        while end < ops.len() && !ops[end].is_equal() {
            end += 1;
        }
        loop {
            let mut gap_end = end;
            while gap_end < ops.len() && ops[gap_end].is_equal() {
                gap_end += 1;
            }
            let gap = gap_end - end;
            if gap_end >= ops.len() {
                end += context.min(gap);
                break;
            }
            if gap <= 2 * context {
                let mut next_end = gap_end;
                while next_end < ops.len() && !ops[next_end].is_equal() {
                    next_end += 1;
                }
                end = next_end;
                continue;
            }
            end += context;
            break;
        }

        hunks.push(Hunk { start, end });
        i = end;
    }
    hunks
}

/// Render a full unified diff of `old` vs `new` to `out`, `diff -u` style:
/// `--- <label>` / `+++ <label>` header (same label both sides — this is one
/// file's before/after, not two files), then one `@@ -a,b +c,d @@` block per
/// hunk with ` `/`-`/`+`-prefixed lines. `context` is the number of unchanged
/// lines shown around each change (3 matches conventional `diff -u`/`git
/// diff`). Writes nothing if `old == new`.
pub fn unified_diff(out: &mut Vec<u8>, label: &str, old: &[u8], new: &[u8], context: usize) {
    if old == new {
        return;
    }

    let old_lines = split_lines(old);
    let new_lines = split_lines(new);

    let ops = diff_ops(&old_lines, &new_lines);
    let hunks = build_hunks(&ops, context);
    if hunks.is_empty() {
        return;
    }

    out.extend_from_slice(format!("--- {label}\n+++ {label}\n").as_bytes());

    for hunk in hunks {
        // Line numbers just before the hunk: count old/new lines consumed by
        // every op before `hunk.start`.
        let mut old_line = 0;
        let mut new_line = 0;
        for op in &ops[..hunk.start] {
            match op {
                Op::Equal(_) => {
                    old_line += 1;
                    new_line += 1;
                }
                Op::Delete(_) => old_line += 1,
                Op::Insert(_) => new_line += 1,
            }
        }

        let mut old_count = 0;
        let mut new_count = 0;
        for op in &ops[hunk.start..hunk.end] {
            match op {
                Op::Equal(_) => {
                    old_count += 1;
                    new_count += 1;
                }
                Op::Delete(_) => old_count += 1,
                Op::Insert(_) => new_count += 1,
            }
        }

        // POSIX unified-diff convention: an empty side reports the line
        // number *before* which the (empty) range sits, with no `+1`.
        let old_start = if old_count > 0 { old_line + 1 } else { old_line };
        let new_start = if new_count > 0 { new_line + 1 } else { new_line };
        out.extend_from_slice(format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@\n").as_bytes());

        for op in &ops[hunk.start..hunk.end] {
            let (c, l) = match op {
                Op::Equal(l) => (b' ', l),
                Op::Delete(l) => (b'-', l),
                Op::Insert(l) => (b'+', l),
            };
            out.push(c);
            out.extend_from_slice(l);
            out.push(b'\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(old: &str, new: &str, context: usize) -> String {
        let mut out = Vec::new();
        unified_diff(&mut out, "f", old.as_bytes(), new.as_bytes(), context);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn identical_text_writes_nothing() {
        crate::languages::init();
        assert_eq!(diff("a\nb\nc\n", "a\nb\nc\n", 3), "");
    }

    #[test]
    fn single_line_change_in_the_middle_full_context() {
        crate::languages::init();
        assert_eq!(diff("a\nb\nc\nd\ne\n", "a\nb\nX\nd\ne\n", 3), "--- f\n+++ f\n@@ -1,5 +1,5 @@\n a\n b\n-c\n+X\n d\n e\n");
    }

    #[test]
    fn two_distant_changes_stay_in_separate_hunks() {
        crate::languages::init();
        let old = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n";
        let new = "1\n2\nX\n4\n5\n6\n7\n8\n9\n10\nY\n12\n";
        assert_eq!(
            diff(old, new, 3),
            "--- f\n+++ f\n@@ -1,6 +1,6 @@\n 1\n 2\n-3\n+X\n 4\n 5\n 6\n@@ -8,5 +8,5 @@\n 8\n 9\n 10\n-11\n+Y\n 12\n"
        );
    }

    #[test]
    fn pure_insertion_at_start_reports_a_zero_count_old_range() {
        crate::languages::init();
        assert_eq!(diff("a\nb\n", "x\na\nb\n", 0), "--- f\n+++ f\n@@ -0,0 +1,1 @@\n+x\n");
    }

    #[test]
    fn pure_insertion_at_start_with_context_pulls_in_trailing_lines() {
        crate::languages::init();
        assert_eq!(diff("a\nb\n", "x\na\nb\n", 3), "--- f\n+++ f\n@@ -1,2 +1,3 @@\n+x\n a\n b\n");
    }

    #[test]
    fn no_trailing_newline_on_either_side() {
        crate::languages::init();
        assert_eq!(diff("a\nb", "a\nX", 3), "--- f\n+++ f\n@@ -1,2 +1,2 @@\n a\n-b\n+X\n");
    }
}
