use crate::document::Document;
use pulldown_cmark::{Event, LinkType, Options, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    ops::Range,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

mod markdown;
mod table;

fn tag_style(style: Style, tag: &Tag<'_>) -> Style {
    match tag {
        Tag::Heading { .. } => style
            .fg(Color::Rgb(151, 212, 185))
            .add_modifier(Modifier::BOLD),
        Tag::Strong => style
            .fg(Color::Rgb(242, 239, 226))
            .add_modifier(Modifier::BOLD),
        Tag::Emphasis => style.add_modifier(Modifier::ITALIC),
        Tag::Strikethrough => style.add_modifier(Modifier::CROSSED_OUT),
        Tag::CodeBlock(_) => style.fg(Color::Rgb(225, 191, 136)),
        Tag::Link { .. } => style
            .fg(Color::Rgb(133, 187, 233))
            .add_modifier(Modifier::UNDERLINED),
        Tag::BlockQuote(_) => style.fg(Color::Rgb(139, 154, 177)),
        _ => style,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    pub text: String,
    pub source: Range<usize>,
    pub style: Style,
    pub width: usize,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub glyphs: Vec<Glyph>,
    pub start: usize,
    pub end: usize,
    pub width: usize,
    pub separator: bool,
}
#[derive(Debug)]
pub struct Layout {
    pub rows: Vec<Row>,
    pub blocks: Vec<Range<usize>>,
    pub active: Vec<Range<usize>>,
    pub width: usize,
    pub images: Vec<ImageSlot>,
    /// Image destinations and local Mermaid render keys in document order.
    pub image_urls: Vec<String>,
    segments: Vec<Segment>,
    parsed: Parsed,
}

/// A row-aligned run of blocks. Each segment begins right after a newline, so it can
/// be laid out on its own and reused while other parts of the note change.
#[derive(Debug)]
struct Segment {
    source: Range<usize>,
    blocks: Range<usize>,
    last: bool,
    active: Vec<Range<usize>>,
    /// Final rows in `Layout::rows`, including `extra` cursor and image rows.
    rows: Range<usize>,
    extra: Vec<usize>,
    /// The fully rendered rows while one of its blocks is shown as source, so moving
    /// the cursor away does not require parsing the note again.
    alt: Option<Vec<Row>>,
}

/// Facts derived from one parse of the text, reused while only the cursor moves.
#[derive(Debug, Default)]
struct Parsed {
    text: String,
    context: u64,
    tables: Vec<bool>,
    /// Per block: the event structure relative to the block start. The same text can
    /// parse differently after an edit elsewhere (a lone `*` is a paragraph after a
    /// reference definition but an empty list item after a heading).
    signatures: Vec<u64>,
    image_refs: Vec<(String, Range<usize>, bool)>,
}

struct Analysis<'a> {
    events: Vec<(Event<'a>, Range<usize>)>,
    block_events: Vec<Range<usize>>,
}

#[derive(Clone, Debug)]
pub struct ImageSlot {
    pub url: String,
    pub source: Range<usize>,
    pub row: usize,
    pub height: usize,
}

/// Add the cursor insertion cell and image rows inside one segment, which occupies
/// `rows[start..]`. Returns the inserted rows relative to `start`.
#[allow(clippy::too_many_arguments)]
fn decorate(
    rows: &mut Vec<Row>,
    start: usize,
    source: &Range<usize>,
    last: bool,
    cursor: usize,
    width: usize,
    image_refs: &[(String, Range<usize>, bool)],
    image_heights: &HashMap<String, usize>,
    active: &[Range<usize>],
    images: &mut Vec<ImageSlot>,
) -> Vec<usize> {
    let mut extra: Vec<usize> = vec![];
    let empty = |at: usize| Row {
        glyphs: vec![],
        start: at,
        end: at,
        width: 0,
        separator: false,
    };
    // Keep an insertion cell at EOF / at a full-width line end.
    if cursor >= source.start
        && (cursor < source.end || last)
        && let Some(i) = (start..rows.len()).find(|&i| {
            rows[i].end == cursor
                && rows[i].width >= width
                && rows.get(i + 1).is_none_or(|next| next.start != cursor)
        })
    {
        rows.insert(i + 1, empty(cursor));
        extra.push(i + 1 - start);
    }
    let first = image_refs.partition_point(|(_, r, _)| r.start < source.start);
    let mut local: Vec<ImageSlot> = vec![];
    for (url, image, in_table) in &image_refs[first..] {
        if image.start >= source.end {
            break;
        }
        if *in_table {
            continue;
        }
        let Some(&height) = image_heights.get(url) else {
            continue;
        };
        if active
            .iter()
            .any(|r| r.start <= image.start && image.end <= r.end + 1)
        {
            continue;
        }
        let Some(row) = rows[start..].iter().rposition(|r| {
            r.glyphs
                .iter()
                .any(|g| g.source.start >= image.start && g.source.start < image.end)
        }) else {
            continue;
        };
        let mut row = row + 1;
        // Several inline images may share a caption row. Keep their source
        // order instead of repeatedly inserting in front of the first image.
        while let Some(slot) = local.iter().find(|slot| slot.row == row) {
            row += slot.height;
        }
        let height = height.clamp(
            1,
            if crate::mermaid::source(url).is_some() {
                crate::mermaid::MAX_ROWS
            } else {
                24
            },
        );
        for slot in &mut local {
            if slot.row >= row {
                slot.row += height;
            }
        }
        for e in &mut extra {
            if *e >= row {
                *e += height;
            }
        }
        rows.splice(
            start + row..start + row,
            (0..height).map(|_| empty(image.start)),
        );
        extra.extend(row..row + height);
        local.push(ImageSlot {
            url: url.clone(),
            source: image.clone(),
            row,
            height,
        });
    }
    extra.sort_unstable();
    images.extend(local.into_iter().map(|mut slot| {
        slot.row += start;
        slot
    }));
    extra
}

/// The innermost link or image destination around `offset`, and whether it is a
/// `[[wiki link]]`.
pub fn link_at(text: &str, offset: usize) -> Option<(String, bool)> {
    let mut found = None;
    for (event, range) in markdown::events(text) {
        if range.start > offset {
            break;
        }
        if let Event::Start(
            Tag::Link {
                dest_url,
                link_type,
                ..
            }
            | Tag::Image {
                dest_url,
                link_type,
                ..
            },
        ) = &event
            && offset <= range.end
        {
            found = Some((
                dest_url.to_string(),
                matches!(link_type, LinkType::WikiLink { .. }),
            ));
        }
    }
    found
}

/// Image or Mermaid enclosing a source position, including active source blocks.
pub fn image_at(text: &str, offset: usize) -> Option<String> {
    let events = markdown::events(text);
    for (event, range) in &events {
        if range.contains(&offset)
            && let Event::Start(Tag::Image { dest_url, .. }) = event
        {
            return Some(dest_url.to_string());
        }
    }
    crate::mermaid::references(&events)
        .into_iter()
        .find(|(_, range, _)| range.contains(&offset))
        .map(|(url, _, _)| url)
}

/// Headings as (plain text, source offset).
pub fn headings(text: &str) -> Vec<(String, usize)> {
    let mut out = vec![];
    let mut current: Option<(String, usize)> = None;
    for (event, range) in markdown::events(text) {
        match event {
            Event::Start(Tag::Heading { .. }) => current = Some((String::new(), range.start)),
            Event::Text(t) | Event::Code(t) => {
                if let Some((s, _)) = &mut current {
                    s.push_str(&t);
                }
            }
            Event::End(TagEnd::Heading(_)) => out.extend(current.take()),
            _ => {}
        }
    }
    out
}

pub fn image_sources(text: &str) -> Vec<String> {
    let events = markdown::events(text);
    let mut sources: Vec<_> = events
        .iter()
        .filter_map(|(e, r)| {
            if let Event::Start(Tag::Image { dest_url, .. }) = e {
                Some((r.start, dest_url.to_string()))
            } else {
                None
            }
        })
        .collect();
    sources.extend(
        crate::mermaid::references(&events)
            .into_iter()
            .map(|(key, r, _)| (r.start, key)),
    );
    sources.sort_by_key(|(start, _)| *start);
    sources.into_iter().map(|(_, key)| key).collect()
}

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
}

pub fn blocks(text: &str) -> Vec<Range<usize>> {
    let events = markdown::events(text);
    block_ranges(text, &events)
}

fn block_ranges(text: &str, events: &[(Event<'_>, Range<usize>)]) -> Vec<Range<usize>> {
    block_spans(text, events)
        .into_iter()
        .map(|(r, _)| r)
        .collect()
}

/// Top-level blocks with the indices of their events. Events are grouped by nesting
/// depth, not by offset: an empty table cell can start where the next block begins.
fn block_spans(
    text: &str,
    events: &[(Event<'_>, Range<usize>)],
) -> Vec<(Range<usize>, Range<usize>)> {
    let mut out: Vec<(Range<usize>, Range<usize>)> = vec![];
    let mut depth = 0usize;
    let mut begin = 0;
    let mut first = 0;
    let mut push = |range: Range<usize>, indices: Range<usize>| {
        if range.end > range.start && out.last().is_none_or(|(b, _)| range.start >= b.end) {
            out.push((range, indices));
        }
    };
    for (i, (event, range)) in events.iter().enumerate() {
        match event {
            Event::Start(_) => {
                if depth == 0 {
                    begin = range.start;
                    first = i;
                }
                depth += 1;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = text[..range.end].trim_end_matches('\n').len();
                    push(begin..end, first..i + 1);
                }
            }
            Event::Rule if depth == 0 => {
                push(
                    range.start..text[..range.end].trim_end_matches('\n').len(),
                    i..i + 1,
                );
            }
            _ => {}
        }
    }
    out
}

struct Builder {
    rows: Vec<Row>,
    current: Row,
    width: usize,
    hanging_indent: usize,
}
impl Builder {
    fn new(width: usize) -> Self {
        Self {
            rows: vec![],
            current: Row {
                glyphs: vec![],
                start: 0,
                end: 0,
                width: 0,
                separator: false,
            },
            width: width.max(2),
            hanging_indent: 0,
        }
    }
    fn newline(&mut self, next: usize) {
        let row = std::mem::replace(
            &mut self.current,
            Row {
                glyphs: vec![],
                start: next,
                end: next,
                width: 0,
                separator: false,
            },
        );
        self.rows.push(row);
    }
    fn glyph(&mut self, text: &str, source: Range<usize>, style: Style) {
        if text == "\n" || text == "\r\n" {
            self.current.end = source.start;
            self.newline(source.end);
            return;
        }
        let shown = if text == "\t" {
            "    ".to_owned()
        } else {
            text.chars()
                .map(|c| if c.is_control() { '�' } else { c })
                .collect()
        };
        let width = UnicodeWidthStr::width(shown.as_str()).max(1);
        if self.current.width + width > self.width && self.current.width > 0 {
            self.newline(source.start);
            let indent = self.hanging_indent.min(self.width.saturating_sub(width));
            self.synthetic(&" ".repeat(indent), source.start, Style::default());
        }
        if self.current.glyphs.is_empty() {
            self.current.start = source.start;
        }
        self.current.end = source.end;
        self.current.width += width;
        self.current.glyphs.push(Glyph {
            text: shown,
            source,
            style,
            width,
        });
    }
    fn raw(&mut self, text: &str, offset: usize, style: Style) {
        for (i, g) in text.grapheme_indices(true) {
            self.glyph(g, offset + i..offset + i + g.len(), style);
        }
    }
    fn mapped(&mut self, shown: &str, original: &str, range: Range<usize>, style: Style) {
        if let Some(i) = original[range.clone()].find(shown) {
            self.raw(shown, range.start + i, style);
        } else {
            for g in shown.graphemes(true) {
                self.glyph(g, range.clone(), style);
            }
        }
    }
    fn synthetic(&mut self, text: &str, at: usize, style: Style) {
        for g in text.graphemes(true) {
            self.glyph(g, at..at, style);
        }
    }
    fn ensure_break(&mut self, at: usize) {
        if !self.current.glyphs.is_empty() {
            self.newline(at);
        }
    }
    fn rendered<'a>(
        &mut self,
        text: &str,
        range: Range<usize>,
        mut events: impl Iterator<Item = (Event<'a>, Range<usize>)>,
    ) {
        let mut styles = vec![Style::default()];
        let mut lists: Vec<Option<u64>> = vec![];
        let mut item_indents: Vec<usize> = vec![];
        let mut image_depth = 0usize;
        while let Some((event, source)) = events.next() {
            let style = *styles.last().unwrap();
            if let Event::Start(ref tag) = event
                && crate::mermaid::is_mermaid(tag)
            {
                self.ensure_break(source.start);
                self.synthetic(
                    "Mermaid · 点击或移入编辑源码",
                    source.start,
                    style.fg(Color::Rgb(151, 212, 185)),
                );
                for (event, _) in events.by_ref() {
                    if matches!(event, Event::End(TagEnd::CodeBlock)) {
                        break;
                    }
                }
                if source.end < range.end {
                    self.ensure_break(source.end);
                }
                continue;
            }
            if let Event::Start(Tag::Table(alignments)) = event {
                self.ensure_break(source.start);
                let indent = self.hanging_indent;
                self.hanging_indent = 0;
                self.table(text, source.clone(), &alignments, &mut events, 0, style);
                self.hanging_indent = indent;
                if source.end < range.end {
                    self.ensure_break(source.end);
                }
                continue;
            }
            match event {
                Event::Start(tag) => {
                    let next = tag_style(style, &tag);
                    match tag {
                        Tag::List(n) => {
                            lists.push(n);
                        }
                        Tag::Item => {
                            self.ensure_break(source.start);
                            let marker = if lists.last().is_some_and(Option::is_some) {
                                let ordered_start =
                                    lists.iter().rposition(Option::is_none).map_or(0, |i| i + 1);
                                let numbers: Vec<_> = lists
                                    .iter()
                                    .enumerate()
                                    .skip(ordered_start)
                                    .map(|(i, n)| {
                                        let n = n.unwrap();
                                        if i + 1 == lists.len() {
                                            n
                                        } else {
                                            n.saturating_sub(1)
                                        }
                                        .to_string()
                                    })
                                    .collect();
                                if let Some(Some(n)) = lists.last_mut() {
                                    *n = n.saturating_add(1);
                                }
                                format!("{}. ", numbers.join("."))
                            } else {
                                format!("{} ", ["•", "◦", "▪"][lists.len().saturating_sub(1) % 3])
                            };
                            let indent = lists.len().saturating_sub(1) * 2;
                            self.hanging_indent = indent + UnicodeWidthStr::width(marker.as_str());
                            item_indents.push(self.hanging_indent);
                            self.synthetic(
                                &format!("{}{marker}", " ".repeat(indent)),
                                source.start,
                                next.fg(Color::Rgb(151, 212, 185)),
                            );
                        }
                        Tag::BlockQuote(_) => {
                            self.synthetic("▎ ", source.start, next);
                        }
                        Tag::Paragraph => {
                            self.ensure_break_unless_marker(source.start);
                            if self.current.glyphs.is_empty() && self.hanging_indent > 0 {
                                self.synthetic(
                                    &" ".repeat(
                                        self.hanging_indent.min(self.width.saturating_sub(2)),
                                    ),
                                    source.start,
                                    next,
                                );
                            }
                        }
                        Tag::Image { .. } => {
                            image_depth += 1;
                            self.synthetic("[图片: ", source.start, next);
                        }
                        Tag::TableHead | Tag::TableRow => {
                            self.ensure_break(source.start);
                        }
                        _ => {}
                    }
                    styles.push(next);
                }
                Event::End(tag) => {
                    styles.pop();
                    match tag {
                        TagEnd::List(_) => {
                            lists.pop();
                        }
                        TagEnd::Item => {
                            item_indents.pop();
                            self.hanging_indent = item_indents.last().copied().unwrap_or(0);
                            if source.end < range.end {
                                self.ensure_break(source.end);
                            }
                        }
                        TagEnd::Paragraph | TagEnd::TableRow | TagEnd::TableHead => {
                            // The final block boundary is supplied by the original newline gap.
                            if source.end < range.end {
                                self.ensure_break(source.end);
                            }
                        }
                        TagEnd::TableCell => {
                            self.synthetic(
                                "  │  ",
                                source.end.saturating_sub(1),
                                style.fg(Color::DarkGray),
                            );
                        }
                        TagEnd::Image => {
                            image_depth = image_depth.saturating_sub(1);
                            self.synthetic("]", source.end.saturating_sub(1), style);
                        }
                        _ => {}
                    }
                }
                Event::Text(s) => {
                    self.mapped(&s, text, source, style);
                }
                Event::Code(s) => {
                    self.mapped(&s, text, source, style.fg(Color::Rgb(225, 191, 136)));
                }
                Event::SoftBreak | Event::HardBreak => {
                    self.glyph("\n", source, style);
                    self.synthetic(
                        &" ".repeat(self.hanging_indent.min(self.width.saturating_sub(2))),
                        self.current.start,
                        style,
                    );
                }
                Event::Rule => {
                    self.synthetic(
                        &"─".repeat(self.width.min(40)),
                        source.start,
                        style.fg(Color::DarkGray),
                    );
                }
                Event::TaskListMarker(checked) => {
                    self.synthetic(if checked { "☑ " } else { "☐ " }, source.start, style);
                }
                Event::Html(s) | Event::InlineHtml(s) => {
                    self.mapped(&s, text, source, style.fg(Color::DarkGray));
                }
                Event::FootnoteReference(s) => {
                    self.synthetic(&format!("[{s}]"), source.start, style);
                }
                _ => {
                    self.raw(&text[source.clone()], source.start, style);
                }
            }
        }
        // A code block Text event includes a final newline before the closing fence.
        if self.current.glyphs.is_empty()
            && self.rows.last().is_some_and(|r| r.start >= range.start)
        {
            self.current = self.rows.pop().unwrap();
        }
        self.current.end = range.end;
        self.hanging_indent = 0;
    }
    fn ensure_break_unless_marker(&mut self, at: usize) {
        if self.current.glyphs.iter().any(|g| !g.source.is_empty()) {
            self.ensure_break(at);
        }
    }
}

fn shift_rows(rows: &mut [Row], delta: isize) {
    if delta == 0 {
        return;
    }
    let move_by = |v: usize| v.wrapping_add_signed(delta);
    for row in rows {
        row.start = move_by(row.start);
        row.end = move_by(row.end);
        for g in &mut row.glyphs {
            g.source = move_by(g.source.start)..move_by(g.source.end);
        }
    }
}

/// Parse once and record which events belong to each top-level block.
fn analyse(text: &str) -> (Analysis<'_>, Vec<Range<usize>>, Parsed) {
    let (events, context) = markdown::parse(text);
    let (blocks, block_events): (Vec<_>, Vec<_>) = block_spans(text, &events).into_iter().unzip();
    let signatures = blocks
        .iter()
        .zip(&block_events)
        .map(|(block, indices)| signature(&events[indices.clone()], block.start))
        .collect();
    let tables = block_events
        .iter()
        .map(|r| matches!(events.get(r.start), Some((Event::Start(Tag::Table(_)), _))))
        .collect();
    let table_ranges: Vec<_> = events
        .iter()
        .filter_map(|(e, r)| matches!(e, Event::Start(Tag::Table(_))).then_some(r.clone()))
        .collect();
    let mut image_refs: Vec<_> = events
        .iter()
        .filter_map(|(e, r)| {
            if let Event::Start(Tag::Image { dest_url, .. }) = e {
                let in_table = table_ranges
                    .iter()
                    .any(|t| t.start <= r.start && r.end <= t.end);
                Some((dest_url.to_string(), r.clone(), in_table))
            } else {
                None
            }
        })
        .collect();
    image_refs.extend(crate::mermaid::references(&events));
    image_refs.sort_by_key(|(_, r, _)| r.start);
    let parsed = Parsed {
        text: text.to_owned(),
        context,
        tables,
        signatures,
        image_refs,
    };
    (
        Analysis {
            events,
            block_events,
        },
        blocks,
        parsed,
    )
}

fn signature(events: &[(Event<'_>, Range<usize>)], base: usize) -> u64 {
    let mut h = DefaultHasher::new();
    for (event, range) in events {
        std::mem::discriminant(event).hash(&mut h);
        (range.start - base, range.end - base).hash(&mut h);
        match event {
            Event::Start(tag) => {
                std::mem::discriminant(tag).hash(&mut h);
                match tag {
                    Tag::Heading { level, .. } => (*level as u8).hash(&mut h),
                    Tag::List(start) => start.hash(&mut h),
                    Tag::Table(alignments) => alignments.len().hash(&mut h),
                    Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                        dest_url.as_ref().hash(&mut h)
                    }
                    _ => {}
                }
            }
            Event::Text(t) | Event::Code(t) | Event::Html(t) | Event::InlineHtml(t) => {
                t.len().hash(&mut h)
            }
            Event::TaskListMarker(done) => done.hash(&mut h),
            _ => {}
        }
    }
    h.finish()
}

/// Split blocks into segments at the last newline between them.
fn segment_shapes(text: &str, blocks: &[Range<usize>]) -> Vec<(Range<usize>, Range<usize>)> {
    let mut out: Vec<(Range<usize>, Range<usize>)> = vec![];
    let mut start = 0;
    let mut first = 0;
    for i in 1..blocks.len() {
        let gap = blocks[i - 1].end..blocks[i].start;
        if let Some(nl) = text[gap.clone()].rfind('\n') {
            let cut = gap.start + nl + 1;
            out.push((start..cut, first..i));
            start = cut;
            first = i;
        }
    }
    out.push((start..text.len(), first..blocks.len()));
    out
}

struct Shape<'a> {
    text: &'a str,
    blocks: &'a [Range<usize>],
    tables: &'a [bool],
    width: usize,
}

impl Shape<'_> {
    /// Lay out one segment exactly as a whole-note pass would at the same position.
    fn render(
        &self,
        source: &Range<usize>,
        blocks: Range<usize>,
        active: &[Range<usize>],
        analysis: Option<&Analysis<'_>>,
        last: bool,
    ) -> Vec<Row> {
        let text = self.text;
        let mut b = Builder::new(self.width);
        b.current.start = source.start;
        b.current.end = source.start;
        let mut pos = source.start;
        for i in blocks {
            let range = self.blocks[i].clone();
            b.raw(&text[pos..range.start], pos, Style::default());
            if active.contains(&range) {
                if !self.tables[i] || !b.table_source(text, range.clone()) {
                    b.raw(&text[range.clone()], range.start, Style::default());
                }
            } else {
                let analysis = analysis.expect("inactive blocks are rendered from parsed events");
                let events = &analysis.events[analysis.block_events[i].clone()];
                b.rendered(text, range.clone(), events.iter().cloned());
            }
            pos = range.end;
        }
        b.raw(&text[pos..source.end], pos, Style::default());
        if last {
            b.rows.push(b.current);
        }
        b.rows
    }
}

impl Layout {
    pub fn new(doc: &Document, width: usize) -> Self {
        Self::with_images(doc, width, &HashMap::new())
    }
    pub fn with_images(
        doc: &Document,
        width: usize,
        image_heights: &HashMap<String, usize>,
    ) -> Self {
        Self::update(None, doc, width, image_heights)
    }
    /// Build a layout, reusing rows of segments whose text, context, width and
    /// active state are unchanged since `previous`. Moving the cursor within the same
    /// text does not parse the note again.
    pub fn update(
        previous: Option<Layout>,
        doc: &Document,
        width: usize,
        image_heights: &HashMap<String, usize>,
    ) -> Self {
        let text = doc.text.as_str();
        let width = width.max(2);
        let same_text = previous.as_ref().is_some_and(|p| p.parsed.text == text);
        let mut analysis = None;
        let (blocks, parsed, shapes, mut old) = match previous {
            Some(p) if same_text => {
                let shapes = p
                    .segments
                    .iter()
                    .map(|s| (s.source.clone(), s.blocks.clone()))
                    .collect();
                (p.blocks.clone(), Parsed::default(), shapes, Some(p))
            }
            previous => {
                let (a, blocks, parsed) = analyse(text);
                analysis = Some(a);
                let shapes = segment_shapes(text, &blocks);
                (blocks, parsed, shapes, previous)
            }
        };
        let parsed = match &mut old {
            Some(p) if same_text => std::mem::take(&mut p.parsed),
            _ => parsed,
        };
        let selection = doc.selection();
        let active: Vec<_> = blocks
            .iter()
            .filter(|r| {
                (doc.cursor >= r.start && doc.cursor <= r.end)
                    || selection
                        .as_ref()
                        .is_some_and(|s| s.start < r.end && s.end > r.start)
            })
            .cloned()
            .collect();

        // Map previous segments onto the new text through the unchanged prefix and suffix.
        // `mapped` is in source order: (new start, new end, old index, offset delta).
        let mut mapped: Vec<(usize, usize, usize, isize)> = vec![];
        let (mut old_rows, mut old_segments, mut old_blocks) = (Vec::new(), Vec::new(), Vec::new());
        let mut old_signatures = Vec::new();
        if let Some(p) = old
            && p.width == width
            && (same_text || p.parsed.context == parsed.context)
        {
            // When the text is unchanged its parse was moved into `parsed`.
            let old_text = if same_text { text } else { &p.parsed.text }.as_bytes();
            let new_text = text.as_bytes();
            let (prefix, suffix) = if same_text {
                (text.len(), text.len())
            } else {
                let prefix = old_text
                    .iter()
                    .zip(new_text)
                    .take_while(|(a, b)| a == b)
                    .count();
                let suffix = old_text
                    .iter()
                    .rev()
                    .zip(new_text.iter().rev())
                    .take(old_text.len().min(new_text.len()))
                    .take_while(|(a, b)| a == b)
                    .count();
                (prefix, suffix)
            };
            let old_len = old_text.len();
            let delta = new_text.len() as isize - old_len as isize;
            for (i, s) in p.segments.iter().enumerate() {
                if s.source.end <= prefix {
                    mapped.push((s.source.start, s.source.end, i, 0));
                } else if s.source.start >= old_len - suffix {
                    let start = s.source.start.wrapping_add_signed(delta);
                    let end = s.source.end.wrapping_add_signed(delta);
                    mapped.push((start, end, i, delta));
                }
            }
            old_rows = p.rows;
            old_segments = p.segments;
            old_blocks = p.blocks;
            // Unchanged text needs no signature comparison.
            if !same_text {
                old_signatures = p.parsed.signatures;
            }
        }
        let capacity = old_rows.len().max(1024);
        let mut old_rows = old_rows.into_iter();
        let mut old_pos = 0;
        // Move a previous segment's rows (without its cursor and image rows) to `out`.
        let mut take_old = |segment: &Segment, out: &mut Vec<Row>, delta: isize| {
            if segment.rows.start > old_pos {
                old_rows.nth(segment.rows.start - old_pos - 1);
            }
            let start = out.len();
            let mut extra = segment.extra.iter().peekable();
            for (i, row) in old_rows.by_ref().take(segment.rows.len()).enumerate() {
                if extra.next_if_eq(&&i).is_none() {
                    out.push(row);
                }
            }
            old_pos = segment.rows.end;
            shift_rows(&mut out[start..], delta);
        };

        let shape = Shape {
            text,
            blocks: &blocks,
            tables: &parsed.tables,
            width,
        };
        let last_index = shapes.len() - 1;
        let mut rows = Vec::with_capacity(capacity);
        let mut segments = Vec::with_capacity(shapes.len());
        let mut images = vec![];
        let mut next_mapped = mapped.into_iter().peekable();
        for (index, (source, block_range)) in shapes.into_iter().enumerate() {
            let last = index == last_index;
            let seg_active: Vec<_> = active
                .iter()
                .filter(|a| a.start >= source.start && a.end <= source.end)
                .cloned()
                .collect();
            let all_active = seg_active.len() == block_range.len();
            let mut alt = None;
            let start = rows.len();
            let mut done = false;
            while next_mapped.next_if(|m| m.0 < source.start).is_some() {}
            // Identical text can still be split into different blocks, or parse into
            // different blocks after an edit elsewhere; compare both before reusing.
            if let Some((_, _, i, delta)) =
                next_mapped.next_if(|m| m.0 == source.start && m.1 == source.end)
                && old_segments[i].last == last
                && old_blocks[old_segments[i].blocks.clone()]
                    .iter()
                    .map(|r| r.start.wrapping_add_signed(delta)..r.end.wrapping_add_signed(delta))
                    .eq(blocks[block_range.clone()].iter().cloned())
                && (same_text
                    || old_signatures[old_segments[i].blocks.clone()]
                        == parsed.signatures[block_range.clone()])
            {
                let old: &mut Segment = &mut old_segments[i];
                let same_active = old
                    .active
                    .iter()
                    .map(|r| r.start.wrapping_add_signed(delta)..r.end.wrapping_add_signed(delta))
                    .eq(seg_active.iter().cloned());
                let mut old_alt = old.alt.take();
                if let Some(alt_rows) = &mut old_alt {
                    shift_rows(alt_rows, delta);
                }
                if same_active {
                    take_old(old, &mut rows, delta);
                    alt = old_alt;
                    done = true;
                } else if seg_active.is_empty()
                    && let Some(alt_rows) = old_alt.take()
                {
                    rows.extend(alt_rows);
                    done = true;
                } else if all_active {
                    alt = if old.active.is_empty() {
                        let mut stash = vec![];
                        take_old(old, &mut stash, delta);
                        Some(stash)
                    } else {
                        old_alt
                    };
                    rows.extend(shape.render(
                        &source,
                        block_range.clone(),
                        &seg_active,
                        None,
                        last,
                    ));
                    done = true;
                }
            }
            if !done {
                let a = &*analysis.get_or_insert_with(|| analyse(text).0);
                if !seg_active.is_empty() {
                    alt = Some(shape.render(&source, block_range.clone(), &[], Some(a), last));
                }
                rows.extend(shape.render(&source, block_range.clone(), &seg_active, Some(a), last));
            }
            let extra = decorate(
                &mut rows,
                start,
                &source,
                last,
                doc.cursor,
                width,
                &parsed.image_refs,
                image_heights,
                &active,
                &mut images,
            );
            segments.push(Segment {
                source,
                blocks: block_range,
                last,
                active: seg_active,
                rows: start..rows.len(),
                extra,
                alt,
            });
        }
        let image_urls = parsed
            .image_refs
            .iter()
            .map(|(u, _, _)| u.clone())
            .collect();
        Self {
            rows,
            blocks,
            active,
            width,
            images,
            image_urls,
            segments,
            parsed,
        }
    }
    /// Segments whose rows can contain `source`, latest first.
    fn candidates(&self, source: usize) -> impl Iterator<Item = &Segment> {
        let i = self
            .segments
            .partition_point(|s| s.source.end <= source && !s.last);
        self.segments.get(i).into_iter()
    }
    pub fn position(&self, source: usize) -> (usize, usize) {
        let ranges: Vec<_> = self.candidates(source).map(|s| s.rows.clone()).collect();
        for rows in ranges
            .iter()
            .cloned()
            .chain(std::iter::once(0..self.rows.len()))
        {
            if let Some(found) = self.position_in(source, rows) {
                return found;
            }
        }
        (0, 0)
    }
    fn position_in(&self, source: usize, rows: Range<usize>) -> Option<(usize, usize)> {
        // Aligned source tables interleave wrapped cells. Actual source glyphs take
        // precedence over the virtual padding and repeated borders around them.
        for y in rows.clone().rev() {
            let mut x = 0;
            for g in &self.rows[y].glyphs {
                if !g.source.is_empty() && source >= g.source.start && source < g.source.end {
                    return Some((y, x));
                }
                x += g.width;
            }
        }
        // Prefer the following row at a soft-wrap boundary.
        for y in rows.rev() {
            let row = &self.rows[y];
            if row.separator || source < row.start || source > row.end {
                continue;
            }
            let mut x = 0;
            for g in &row.glyphs {
                if source <= g.source.start || (source < g.source.end && !g.source.is_empty()) {
                    return Some((y, x));
                }
                x += g.width;
            }
            return Some((y, x.min(self.width.saturating_sub(1))));
        }
        None
    }
    pub fn hit(&self, y: usize, x: usize) -> usize {
        let row = &self.rows[y.min(self.rows.len() - 1)];
        let mut col = 0;
        for g in &row.glyphs {
            if x < col + g.width {
                return g.source.start;
            }
            col += g.width;
        }
        row.end
    }
    pub fn vertical_hit(&self, from: usize, target: usize, x: usize) -> usize {
        let mut y = target.min(self.rows.len() - 1);
        let down = target >= from;
        while self.rows[y].separator {
            if down && y + 1 < self.rows.len() {
                y += 1;
            } else if !down && y > 0 {
                y -= 1;
            } else {
                return self.hit(from, x);
            }
        }
        self.hit(y, x)
    }
    pub fn plain(&self) -> String {
        self.rows
            .iter()
            .map(|r| r.glyphs.iter().map(|g| g.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }
}
