//! Rules on the shared body, `instructions/shared.md` (spec Rules): its size
//! against the smallest instruction size limit among enabled tools, and
//! `@` tokens, which Claude Code reads as file imports.

use std::collections::BTreeSet;
use std::io;

use super::config::Records;
use super::{Violation, readable, rule};
use crate::tree::{Kind, Tree};

pub const PATH: &str = "instructions/shared.md";

/// The size rule and the `@` rule.
pub fn check(tree: &dyn Tree, records: &Records, out: &mut BTreeSet<Violation>) -> io::Result<()> {
    let Some(kind @ Kind::File { size, .. }) = tree.kind(PATH)? else {
        return Ok(());
    };
    if let Some((limit, tools)) = size_limit(records)
        && size > limit
    {
        out.insert(Violation::new(
            PATH,
            None,
            rule::SHARED_BODY_SIZE,
            format!(
                "{size} bytes is over the limit of {limit} bytes (the smallest instruction size limit among enabled tools: {tools})"
            ),
        ));
    }
    if readable(kind) {
        let text = tree.read(PATH)?;
        for (line, token) in at_tokens(&String::from_utf8_lossy(&text)) {
            out.insert(Violation::new(
                PATH,
                Some(line),
                rule::AT_IMPORT,
                format!(
                    "`{token}` is read as a file import by Claude Code only; put it in inline code or rewrite it"
                ),
            ));
        }
    }
    Ok(())
}

/// The warning at 80 % of the size limit (up to and including the limit).
pub fn warnings(
    tree: &dyn Tree,
    records: &Records,
    out: &mut BTreeSet<Violation>,
) -> io::Result<()> {
    let (Some(Kind::File { size, .. }), Some((limit, tools))) =
        (tree.kind(PATH)?, size_limit(records))
    else {
        return Ok(());
    };
    if size <= limit && size * 5 >= limit * 4 {
        out.insert(Violation::new(
            PATH,
            None,
            rule::SHARED_BODY_SIZE,
            format!(
                "{size} bytes is at least 80 % of the limit of {limit} bytes (the smallest instruction size limit among enabled tools: {tools})"
            ),
        ));
    }
    Ok(())
}

/// The smallest instruction size limit among enabled tools, with the ids of
/// the tools that have it. `None` when no enabled tool has a known limit or
/// `skiman.toml` does not parse.
fn size_limit(records: &Records) -> Option<(u64, String)> {
    let enabled = records.enabled_tools()?;
    let tools = crate::tools::TOOLS
        .iter()
        .filter(|tool| enabled.iter().any(|id| id == tool.id));
    let limit = tools
        .clone()
        .filter_map(|t| t.instruction_size_limit)
        .min()?;
    let ids: Vec<&str> = tools
        .filter(|t| t.instruction_size_limit == Some(limit))
        .map(|t| t.id)
        .collect();
    Some((limit, ids.join(", ")))
}

/// Every token that starts with `@` at line start or after whitespace,
/// outside fenced code blocks and inline code spans, with its 1-based line.
fn at_tokens(text: &str) -> Vec<(usize, String)> {
    let mut tokens = Vec::new();
    // Lines outside fenced code blocks, grouped into blocks separated by
    // blank lines (a code span never crosses a block).
    let mut block: Vec<(usize, &str)> = Vec::new();
    let mut fence: Option<(u8, usize)> = None;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        match fence {
            Some((ch, len)) => {
                if closes_fence(line, ch, len) {
                    fence = None;
                }
                continue;
            }
            None => {
                if let Some(open) = opens_fence(line) {
                    scan_block(&block, &mut tokens);
                    block.clear();
                    fence = Some(open);
                    continue;
                }
            }
        }
        if line.trim().is_empty() {
            scan_block(&block, &mut tokens);
            block.clear();
        } else {
            block.push((number, line));
        }
    }
    scan_block(&block, &mut tokens);
    tokens
}

/// Up to three spaces of indentation, then the fence characters.
fn fence_run(line: &str) -> Option<(u8, usize, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let ch = *rest.as_bytes().first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let len = rest.len() - rest.trim_start_matches(ch as char).len();
    (len >= 3).then(|| (ch, len, &rest[len..]))
}

fn opens_fence(line: &str) -> Option<(u8, usize)> {
    let (ch, len, info) = fence_run(line)?;
    // A backtick fence's info string cannot contain a backtick.
    (ch == b'~' || !info.contains('`')).then_some((ch, len))
}

fn closes_fence(line: &str, ch: u8, len: usize) -> bool {
    fence_run(line).is_some_and(|(c, l, rest)| c == ch && l >= len && rest.trim().is_empty())
}

/// Finds `@` tokens in one block of consecutive non-blank lines.
fn scan_block(block: &[(usize, &str)], tokens: &mut Vec<(usize, String)>) {
    let mut text = String::new();
    let mut starts = Vec::new();
    for (number, line) in block {
        starts.push((text.len(), *number));
        text.push_str(line);
        text.push('\n');
    }
    let bytes = text.as_bytes();
    let line_of = |at: usize| {
        starts
            .iter()
            .rev()
            .find(|(start, _)| *start <= at)
            .map_or(0, |(_, n)| *n)
    };
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if bytes.get(i + 1) == Some(&b'`') => i += 2,
            b'`' => {
                let run = backtick_run(bytes, i);
                i = closing_run(bytes, i + run, run).unwrap_or(i + run);
            }
            b'@' if text[..i]
                .chars()
                .next_back()
                .is_none_or(char::is_whitespace) =>
            {
                let end = text[i..]
                    .find(char::is_whitespace)
                    .map_or(text.len(), |n| i + n);
                tokens.push((line_of(i), text[i..end].to_owned()));
                i = end;
            }
            _ => i += 1,
        }
    }
}

fn backtick_run(bytes: &[u8], at: usize) -> usize {
    bytes[at..].iter().take_while(|&&b| b == b'`').count()
}

/// The position after the next run of exactly `run` backticks from `from`.
fn closing_run(bytes: &[u8], from: usize, run: usize) -> Option<usize> {
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let len = backtick_run(bytes, i);
            if len == run {
                return Some(i + len);
            }
            i += len;
        } else {
            i += 1;
        }
    }
    None
}
