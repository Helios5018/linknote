use std::{
    collections::VecDeque,
    ops::Range,
    time::{Duration, Instant},
};
use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

const MAX_UNDO_GROUPS: usize = 1000;
const MAX_UNDO_BYTES: usize = 16 * 1024 * 1024;

/// One replacement, enough to apply it in either direction.
#[derive(Clone, Debug)]
struct Edit {
    at: usize,
    removed: String,
    inserted: String,
}

#[derive(Clone, Debug)]
struct Group {
    edits: Vec<Edit>,
    before: (usize, Option<usize>),
    after: (usize, Option<usize>),
}

impl Group {
    fn bytes(&self) -> usize {
        self.edits
            .iter()
            .map(|e| e.removed.len() + e.inserted.len())
            .sum()
    }
}

/// All offsets are UTF-8 byte offsets at grapheme boundaries.
#[derive(Debug)]
pub struct Document {
    pub text: String,
    pub cursor: usize,
    pub anchor: Option<usize>,
    pub revision: u64,
    undo: VecDeque<Group>,
    undo_bytes: usize,
    redo: Vec<Group>,
    last_edit: Option<(Instant, u8)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Class {
    Space,
    Punct,
    Cjk,
    Word,
}

fn class(g: &str) -> Class {
    let c = g.chars().next().unwrap_or(' ');
    let u = c as u32;
    if c.is_whitespace() {
        Class::Space
    } else if matches!(u, 0x3000..=0x303F | 0xFE30..=0xFE4F | 0x2000..=0x206F)
        || matches!(u, 0xFF00..=0xFF0F | 0xFF1A..=0xFF20 | 0xFF3B..=0xFF40 | 0xFF5B..=0xFF65)
    {
        Class::Punct
    } else if matches!(
        u,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
            | 0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F | 0x20000..=0x3FFFF
    ) {
        Class::Cjk
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

/// A list marker at the start of a line: (indent bytes, marker bytes including the space).
pub fn list_marker(line: &str) -> Option<(usize, usize)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let rest = &line[indent..];
    if ["- ", "* ", "+ "].iter().any(|m| rest.starts_with(m)) {
        return Some((indent, 2));
    }
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=9).contains(&digits)
        && (rest[digits..].starts_with(". ") || rest[digits..].starts_with(") "))
    {
        return Some((indent, digits + 2));
    }
    None
}

impl Document {
    pub fn new(text: String) -> Self {
        Self {
            text,
            cursor: 0,
            anchor: None,
            revision: 0,
            undo: VecDeque::new(),
            undo_bytes: 0,
            redo: vec![],
            last_edit: None,
        }
    }
    fn push_undo(&mut self, group: Group) {
        self.undo_bytes += group.bytes();
        self.undo.push_back(group);
        while self.undo.len() > 1
            && (self.undo.len() > MAX_UNDO_GROUPS || self.undo_bytes > MAX_UNDO_BYTES)
        {
            let old = self.undo.pop_front().unwrap();
            self.undo_bytes -= old.bytes();
        }
    }
    pub fn selection(&self) -> Option<Range<usize>> {
        self.anchor
            .filter(|a| *a != self.cursor)
            .map(|a| a.min(self.cursor)..a.max(self.cursor))
    }
    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|r| &self.text[r])
    }
    pub fn break_group(&mut self) {
        self.last_edit = None;
    }
    /// The grapheme boundary at or before `offset`. Only nearby text is inspected.
    pub fn boundary(&self, offset: usize) -> usize {
        if offset >= self.text.len() {
            return self.text.len();
        }
        let mut offset = offset;
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        let mut cursor = GraphemeCursor::new(offset, self.text.len(), true);
        match cursor.is_boundary(&self.text, 0) {
            Ok(true) => offset,
            _ => cursor
                .prev_boundary(&self.text, 0)
                .ok()
                .flatten()
                .unwrap_or(0),
        }
    }
    pub fn set_cursor(&mut self, offset: usize, select: bool) {
        if select {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = self.boundary(offset);
        self.break_group();
    }
    pub fn previous(&self, at: usize) -> usize {
        GraphemeCursor::new(at, self.text.len(), true)
            .prev_boundary(&self.text, 0)
            .ok()
            .flatten()
            .unwrap_or(0)
    }
    pub fn next(&self, at: usize) -> usize {
        GraphemeCursor::new(at, self.text.len(), true)
            .next_boundary(&self.text, 0)
            .ok()
            .flatten()
            .unwrap_or(at)
    }
    fn grapheme_at(&self, at: usize) -> &str {
        &self.text[at..self.next(at)]
    }
    /// Word boundaries without a dictionary: spaces, punctuation, CJK runs and
    /// other word characters form separate runs, so Chinese text stops at
    /// punctuation and script changes instead of jumping to the end of a paragraph.
    pub fn word_boundary(&self, from: usize, right: bool) -> usize {
        let mut to = from;
        if right {
            while to < self.text.len() && class(self.grapheme_at(to)) == Class::Space {
                to = self.next(to);
            }
            if to < self.text.len() {
                let kind = class(self.grapheme_at(to));
                while to < self.text.len() && class(self.grapheme_at(to)) == kind {
                    to = self.next(to);
                }
            }
        } else {
            let before = |to: usize| &self.text[self.previous(to)..to];
            while to > 0 && class(before(to)) == Class::Space {
                to = self.previous(to);
            }
            if to > 0 {
                let kind = class(before(to));
                while to > 0 && class(before(to)) == kind {
                    to = self.previous(to);
                }
            }
        }
        to
    }
    pub fn horizontal(&mut self, right: bool, select: bool, word: bool) {
        if !select
            && !word
            && let Some(r) = self.selection()
        {
            self.set_cursor(if right { r.end } else { r.start }, false);
            return;
        }
        let to = if word {
            self.word_boundary(self.cursor, right)
        } else if right {
            self.next(self.cursor)
        } else {
            self.previous(self.cursor)
        };
        self.set_cursor(to, select);
    }
    pub fn line_start(&self) -> usize {
        self.line_start_at(self.cursor)
    }
    fn line_start_at(&self, at: usize) -> usize {
        self.text[..at].rfind('\n').map_or(0, |i| i + 1)
    }
    pub fn line_end(&self) -> usize {
        self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i)
    }
    pub fn line_column(&self) -> (usize, usize) {
        (
            self.text[..self.cursor]
                .bytes()
                .filter(|b| *b == b'\n')
                .count()
                + 1,
            self.text[self.line_start()..self.cursor]
                .graphemes(true)
                .count()
                + 1,
        )
    }
    pub fn insert(&mut self, text: &str) {
        let range = self.selection().unwrap_or(self.cursor..self.cursor);
        let kind = if self.selection().is_none()
            && !text.contains('\n')
            && text.graphemes(true).count() == 1
        {
            1
        } else {
            0
        };
        self.replace(range, text, kind);
    }
    fn replace(&mut self, range: Range<usize>, text: &str, kind: u8) {
        if range.is_empty() && text.is_empty() {
            return;
        }
        let group = kind != 0
            && !self.undo.is_empty()
            && self
                .last_edit
                .is_some_and(|(t, k)| k == kind && t.elapsed() < Duration::from_millis(600));
        let before = (self.cursor, self.anchor);
        let edit = Edit {
            at: range.start,
            removed: self.text[range.clone()].to_owned(),
            inserted: text.to_owned(),
        };
        self.redo.clear();
        self.text.replace_range(range.clone(), text);
        self.cursor = range.start + text.len();
        // An inserted combining mark may merge with a neighbouring grapheme.
        if self.cursor < self.text.len() && self.boundary(self.cursor) != self.cursor {
            self.cursor = self.next(self.boundary(self.cursor));
        }
        self.anchor = None;
        self.revision += 1;
        self.last_edit = Some((Instant::now(), kind));
        let after = (self.cursor, self.anchor);
        if group {
            let bytes = edit.removed.len() + edit.inserted.len();
            let last = self.undo.back_mut().unwrap();
            last.edits.push(edit);
            last.after = after;
            self.undo_bytes += bytes;
        } else {
            self.push_undo(Group {
                edits: vec![edit],
                before,
                after,
            });
        }
    }
    /// Move the cursor after an edit and make redo restore the same place.
    fn place_after_edit(&mut self, cursor: usize, anchor: Option<usize>) {
        self.cursor = self.boundary(cursor);
        self.anchor = anchor.map(|a| self.boundary(a));
        if let Some(last) = self.undo.back_mut() {
            last.after = (self.cursor, self.anchor);
        }
    }
    /// Replace the whole text as one undoable step, storing only the changed middle.
    pub fn replace_all(&mut self, text: String) {
        if text == self.text {
            return;
        }
        let cursor = self.cursor;
        let old = self.text.as_bytes();
        let new = text.as_bytes();
        let mut prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
        while !self.text.is_char_boundary(prefix) || !text.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let max_suffix = old.len().min(new.len()) - prefix;
        let mut suffix = old
            .iter()
            .rev()
            .zip(new.iter().rev())
            .take(max_suffix)
            .take_while(|(a, b)| a == b)
            .count();
        while !self.text.is_char_boundary(old.len() - suffix)
            || !text.is_char_boundary(new.len() - suffix)
        {
            suffix -= 1;
        }
        self.replace(
            prefix..old.len() - suffix,
            &text[prefix..new.len() - suffix],
            0,
        );
        self.place_after_edit(cursor.min(self.text.len()), None);
        self.break_group();
    }
    pub fn backspace(&mut self) {
        let r = self
            .selection()
            .unwrap_or(self.previous(self.cursor)..self.cursor);
        self.replace(r, "", 2);
    }
    pub fn delete_word_back(&mut self) {
        let r = self
            .selection()
            .unwrap_or(self.word_boundary(self.cursor, false)..self.cursor);
        self.replace(r, "", 0);
    }
    pub fn delete(&mut self) {
        let r = self
            .selection()
            .unwrap_or(self.cursor..self.next(self.cursor));
        self.replace(r, "", 0);
    }
    pub fn undo(&mut self) {
        if let Some(group) = self.undo.pop_back() {
            self.undo_bytes -= group.bytes();
            for e in group.edits.iter().rev() {
                self.text
                    .replace_range(e.at..e.at + e.inserted.len(), &e.removed);
            }
            self.cursor = group.before.0.min(self.text.len());
            self.anchor = group.before.1.map(|a| a.min(self.text.len()));
            self.redo.push(group);
            self.revision += 1;
            self.break_group();
        }
    }
    pub fn redo(&mut self) {
        if let Some(group) = self.redo.pop() {
            for e in &group.edits {
                self.text
                    .replace_range(e.at..e.at + e.removed.len(), &e.inserted);
            }
            self.cursor = group.after.0.min(self.text.len());
            self.anchor = group.after.1.map(|a| a.min(self.text.len()));
            self.undo_bytes += group.bytes();
            self.undo.push_back(group);
            self.revision += 1;
            self.break_group();
        }
    }
    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.text.len();
        self.break_group();
    }
    /// Accept a version from disk. It is one undo step, so the previous text can be recovered.
    pub fn reload(&mut self, text: String) {
        self.replace_all(text);
    }
    /// The byte ranges of the lines touched by the cursor or selection.
    fn touched_lines(&self) -> Range<usize> {
        let r = self.selection().unwrap_or(self.cursor..self.cursor);
        let end = if r.end > r.start && self.text[..r.end].ends_with('\n') {
            r.end - 1
        } else {
            r.end
        };
        let start = self.line_start_at(r.start);
        let end = self.text[end..]
            .find('\n')
            .map_or(self.text.len(), |i| end + i);
        start..end
    }
    /// Indent or outdent list items (and, with a selection, plain lines).
    /// Returns false when Tab should insert spaces instead.
    pub fn indent(&mut self, outdent: bool) -> bool {
        let lines = self.touched_lines();
        let multi = self.text[lines.clone()].contains('\n');
        let first = &self.text[lines.start..];
        let first = &first[..first.find('\n').unwrap_or(first.len())];
        if !outdent && !multi && list_marker(first).is_none() {
            return false;
        }
        let mut out = String::new();
        let mut deltas: Vec<(usize, usize, isize)> = vec![]; // (line start, old indent, delta)
        let mut at = lines.start;
        for line in self.text[lines.clone()].split('\n') {
            let indent = line.len() - line.trim_start_matches(' ').len();
            let target = if line.trim().is_empty() {
                indent
            } else if let Some((indent, width)) = list_marker(line) {
                if outdent {
                    self.parent_indent(at, indent)
                } else {
                    self.sibling_content(at, indent).unwrap_or(indent + width)
                }
            } else if outdent {
                indent.saturating_sub(4)
            } else {
                indent + 4
            };
            out.push_str(&" ".repeat(target));
            out.push_str(&line[indent..]);
            out.push('\n');
            deltas.push((at, indent, target as isize - indent as isize));
            at += line.len() + 1;
        }
        out.pop();
        if out == self.text[lines.clone()] {
            return true;
        }
        let map = |offset: usize| -> usize {
            let mut shift = 0isize;
            for &(start, indent, delta) in &deltas {
                if offset < start {
                    break;
                }
                if offset < start + indent {
                    // Inside the removed or added indentation: keep at its new end.
                    return (start as isize + shift + (indent as isize + delta).max(0)) as usize;
                }
                shift += delta;
            }
            (offset as isize + shift) as usize
        };
        let (cursor, anchor) = (map(self.cursor), self.anchor.map(map));
        self.replace(lines, &out, 0);
        self.place_after_edit(cursor, anchor);
        self.break_group();
        true
    }
    /// Lines before `line_start`, nearest first, stopping at a paragraph boundary.
    fn lines_before(&self, line_start: usize) -> impl Iterator<Item = (usize, &str)> {
        let mut end = line_start;
        std::iter::from_fn(move || {
            if end == 0 {
                return None;
            }
            let stop = end - 1;
            let start = self.text[..stop].rfind('\n').map_or(0, |i| i + 1);
            end = start;
            Some((start, &self.text[start..stop]))
        })
        .take(500)
    }
    /// Indentation of the content of the previous item at the same level, so Tab
    /// nests the current item under it.
    fn sibling_content(&self, line_start: usize, indent: usize) -> Option<usize> {
        for (_, line) in self.lines_before(line_start) {
            match list_marker(line) {
                Some((i, width)) if i == indent => return Some(i + width),
                Some((i, _)) if i < indent => return None,
                None if line.trim().is_empty() || !line.starts_with(' ') => return None,
                _ => {}
            }
        }
        None
    }
    fn parent_indent(&self, line_start: usize, indent: usize) -> usize {
        for (_, line) in self.lines_before(line_start) {
            match list_marker(line) {
                Some((i, _)) if i < indent => return i,
                None if !line.trim().is_empty() && !line.starts_with(' ') => break,
                _ => {}
            }
        }
        0
    }
    pub fn newline(&mut self) {
        if self.selection().is_some() {
            self.insert("\n");
            return;
        }
        let start = self.line_start();
        let line = &self.text[start..self.cursor];
        let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
        let rest = &line[indent_len..];
        let mut prefix = line[..indent_len].to_owned();
        let quote = rest.starts_with("> ");
        if let Some((_, width)) = list_marker(line).filter(|_| !quote) {
            let marker = &rest[..width];
            let task = ["[ ] ", "[x] ", "[X] "]
                .iter()
                .any(|t| rest[width..].starts_with(t));
            let body = &rest[width + if task { 4 } else { 0 }..];
            if body.is_empty() {
                // An empty nested item steps out one level; at the top it ends the list.
                if indent_len > 0 && self.indent(true) {
                    return;
                }
                self.replace(start..self.cursor, "", 0);
                return;
            }
            let digits = marker.bytes().take_while(u8::is_ascii_digit).count();
            if digits > 0 {
                let n: u64 = marker[..digits].parse().unwrap_or(0);
                prefix.push_str(&format!("{}{}", n.saturating_add(1), &marker[digits..]));
            } else {
                prefix.push_str(marker);
            }
            if task {
                prefix.push_str("[ ] ");
            }
        } else if quote {
            if rest == "> " {
                self.replace(start..self.cursor, "", 0);
                return;
            }
            prefix.push_str("> ");
        }
        self.insert(&format!("\n{prefix}"));
    }
}
