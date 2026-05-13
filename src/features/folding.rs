//! Folding ranges feature — brace-balanced and comment block folding.

use tower_lsp::lsp_types::{FoldingRange, FoldingRangeKind, Url};

use super::traits::DocumentAccess;

/// Compute folding ranges for `uri`.
///
/// Returns brace-balanced block ranges and consecutive `//` comment blocks.
/// Returns `None` when the file has no lines or no foldable regions.
pub(crate) fn compute_folding_ranges(
    uri: &Url,
    index: &impl DocumentAccess,
) -> Option<Vec<FoldingRange>> {
    let lines = index.mem_lines_for(uri.as_str())?;
    let mut ranges: Vec<FoldingRange> = Vec::new();
    let mut stack: Vec<u32> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let opens = trimmed.chars().filter(|&c| c == '{').count() as i32;
        let closes = trimmed.chars().filter(|&c| c == '}').count() as i32;
        let net = opens - closes;

        if net > 0 {
            for _ in 0..net {
                stack.push(i as u32);
            }
        } else if net < 0 {
            for _ in 0..(-net) {
                if let Some(start_line) = stack.pop() {
                    if i as u32 > start_line + 1 {
                        ranges.push(FoldingRange {
                            start_line,
                            end_line: i as u32,
                            start_character: None,
                            end_character: None,
                            kind: Some(FoldingRangeKind::Region),
                            collapsed_text: None,
                        });
                    }
                }
            }
        }
    }

    // Fold consecutive comment blocks (lines starting with `//`).
    let mut comment_start: Option<u32> = None;
    for (i, line) in lines.iter().enumerate() {
        if line.trim().starts_with("//") {
            if comment_start.is_none() {
                comment_start = Some(i as u32);
            }
        } else if let Some(cs) = comment_start.take() {
            if i as u32 > cs + 1 {
                ranges.push(FoldingRange {
                    start_line: cs,
                    end_line: (i as u32) - 1,
                    start_character: None,
                    end_character: None,
                    kind: Some(FoldingRangeKind::Comment),
                    collapsed_text: None,
                });
            }
        }
    }

    if ranges.is_empty() {
        None
    } else {
        Some(ranges)
    }
}
