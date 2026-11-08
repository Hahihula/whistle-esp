//! Laying out transcripts.

use alloc::{string::String, vec::Vec};

/// Greedy word wrap into at most `max_lines` lines, line `i` holding up to
/// `cols(i)` characters (constant on a rectangular screen, the chord's
/// width on a round one). Text that doesn't fit ends with "...".
pub fn wrap(text: &str, max_lines: usize, cols: impl Fn(usize) -> usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let need = cur.chars().count() + usize::from(!cur.is_empty()) + word.chars().count();
        if need <= cols(lines.len()) || cur.is_empty() {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
        } else {
            lines.push(core::mem::take(&mut cur));
            cur.push_str(word);
            if lines.len() == max_lines {
                if let Some(last) = lines.last_mut() {
                    last.push_str("...");
                }
                return lines;
            }
        }
    }
    if !cur.is_empty() && lines.len() < max_lines {
        lines.push(cur);
    }
    lines
}
