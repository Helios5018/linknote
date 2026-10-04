//! Text matching for in-note find/replace and the background full-text search.
use crate::{storage, tree};
use std::{
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Smart case: a query without uppercase letters matches case-insensitively.
pub fn insensitive(query: &str) -> bool {
    !query.chars().any(char::is_uppercase)
}

fn fold_eq(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

/// End of a match of `query` starting at byte `at`, if any.
fn match_at(text: &str, at: usize, query: &str, insensitive: bool) -> Option<usize> {
    if !insensitive {
        return text[at..].starts_with(query).then(|| at + query.len());
    }
    if query.is_ascii() {
        let end = at + query.len();
        return (text.len() >= end
            && text.is_char_boundary(end)
            && text.as_bytes()[at..end].eq_ignore_ascii_case(query.as_bytes()))
        .then_some(end);
    }
    let mut chars = text[at..].char_indices();
    for q in query.chars() {
        let (_, c) = chars.next()?;
        if !fold_eq(c, q) {
            return None;
        }
    }
    Some(chars.next().map_or(text.len(), |(i, _)| at + i))
}

/// Matches in `text[range]`, in order, non-overlapping.
fn matches_in(
    text: &str,
    query: &str,
    insensitive: bool,
    range: Range<usize>,
) -> impl Iterator<Item = Range<usize>> {
    let mut at = range.start;
    let first = query.chars().next();
    std::iter::from_fn(move || {
        let first = first?;
        while at < range.end {
            if !insensitive {
                let i = text[at..range.end].find(query)?;
                let start = at + i;
                at = start + query.len();
                return Some(start..at);
            }
            // Skip quickly to a plausible first character.
            let offset = if first.is_ascii() {
                let lower = first.to_ascii_lowercase() as u8;
                text.as_bytes()[at..range.end]
                    .iter()
                    .position(|b| b.to_ascii_lowercase() == lower || *b >= 0x80)?
            } else {
                0
            };
            let start = at + offset;
            if start >= range.end {
                return None;
            }
            if !text.is_char_boundary(start) {
                at = start + 1;
                continue;
            }
            if let Some(end) = match_at(text, start, query, true)
                && end <= range.end
            {
                at = end;
                return Some(start..end);
            }
            at = start + text[start..].chars().next().map_or(1, char::len_utf8);
        }
        None
    })
}

pub fn find_all(text: &str, query: &str) -> Vec<Range<usize>> {
    matches_in(text, query, insensitive(query), 0..text.len()).collect()
}

/// The next match at or after `from`, wrapping to the start of the text.
pub fn find_next(text: &str, query: &str, from: usize) -> Option<Range<usize>> {
    let case = insensitive(query);
    matches_in(text, query, case, from..text.len())
        .next()
        .or_else(|| matches_in(text, query, case, 0..text.len()).next())
}

/// Whether `text[range]` is exactly a match (used before replacing the selection).
pub fn is_match(text: &str, query: &str, range: Range<usize>) -> bool {
    !query.is_empty() && match_at(text, range.start, query, insensitive(query)) == Some(range.end)
}

/// Replace every match; returns the new text and the number of replacements.
pub fn replace_all(text: &str, query: &str, with: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut count = 0;
    for m in find_all(text, query) {
        out.push_str(&text[last..m.start]);
        out.push_str(with);
        last = m.end;
        count += 1;
    }
    out.push_str(&text[last..]);
    (out, count)
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub path: PathBuf,
    pub line: usize,
    pub range: Range<usize>,
    pub snippet: String,
}

/// A short single-line excerpt around a match, at most `width` display cells.
fn snippet(line: &str, start: usize, width: usize) -> String {
    let before: Vec<&str> = line[..start].graphemes(true).collect();
    let mut out = String::new();
    let mut used = 0;
    let mut first = before.len();
    // Keep up to a third of the width before the match.
    while first > 0 && used + UnicodeWidthStr::width(before[first - 1]) <= width / 3 {
        first -= 1;
        used += UnicodeWidthStr::width(before[first]);
    }
    if first > 0 {
        out.push('…');
        used += 1;
    }
    for g in before[first..]
        .iter()
        .copied()
        .chain(line[start..].graphemes(true))
    {
        let w = UnicodeWidthStr::width(g);
        if used + w > width {
            out.push('…');
            break;
        }
        out.push_str(if g == "\t" { " " } else { g });
        used += w;
    }
    out.trim_start().to_owned()
}

pub fn search_text(path: &Path, text: &str, query: &str, limit: usize) -> Vec<Hit> {
    let case = insensitive(query);
    let mut hits = vec![];
    let mut line = 1;
    let mut counted = 0;
    for m in matches_in(text, query, case, 0..text.len()).take(limit) {
        line += text[counted..m.start]
            .bytes()
            .filter(|b| *b == b'\n')
            .count();
        counted = m.start;
        let line_start = text[..m.start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = text[m.start..]
            .find('\n')
            .map_or(text.len(), |i| m.start + i);
        hits.push(Hit {
            path: path.to_path_buf(),
            line,
            range: m.clone(),
            snippet: snippet(&text[line_start..line_end], m.start - line_start, 80),
        });
    }
    hits
}

pub const MAX_HITS: usize = 300;

struct Request {
    generation: u64,
    root: PathBuf,
    query: String,
    /// The open note's unsaved text replaces its disk content.
    current: Option<(PathBuf, String)>,
}

pub struct Results {
    pub query: String,
    pub hits: Vec<Hit>,
    pub truncated: bool,
}

/// Searches note files on a background thread; newer queries cancel older ones.
pub struct Worker {
    tx: mpsc::Sender<Request>,
    rx: mpsc::Receiver<(u64, Results)>,
    generation: Arc<AtomicU64>,
}

impl Default for Worker {
    fn default() -> Self {
        Self::new()
    }
}

impl Worker {
    pub fn new() -> Self {
        let (tx, requests) = mpsc::channel::<Request>();
        let (done, rx) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let latest = generation.clone();
        std::thread::spawn(move || {
            while let Ok(mut request) = requests.recv() {
                while let Ok(newer) = requests.try_recv() {
                    request = newer;
                }
                let stale = || latest.load(Ordering::Relaxed) != request.generation;
                let mut hits = vec![];
                let mut truncated = false;
                let notes = tree::all_notes(&request.root).unwrap_or_default();
                for path in notes {
                    if stale() {
                        break;
                    }
                    let text = match &request.current {
                        Some((p, text)) if *p == path => text.clone(),
                        _ => match storage::read_note(&path) {
                            Ok(bytes) => storage::decode(&bytes).0,
                            Err(_) => continue,
                        },
                    };
                    hits.extend(search_text(
                        &path,
                        &text,
                        &request.query,
                        MAX_HITS - hits.len(),
                    ));
                    if hits.len() >= MAX_HITS {
                        truncated = true;
                        break;
                    }
                }
                if !stale() {
                    let results = Results {
                        query: request.query,
                        hits,
                        truncated,
                    };
                    if done.send((request.generation, results)).is_err() {
                        break;
                    }
                }
            }
        });
        Self { tx, rx, generation }
    }
    pub fn request(&self, root: &Path, query: &str, current: Option<(PathBuf, String)>) {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.tx.send(Request {
            generation,
            root: root.to_path_buf(),
            query: query.to_owned(),
            current,
        });
    }
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
    pub fn poll(&self) -> Option<Results> {
        let mut latest = None;
        while let Ok((generation, results)) = self.rx.try_recv() {
            if generation == self.generation.load(Ordering::Relaxed) {
                latest = Some(results);
            }
        }
        latest
    }
}
