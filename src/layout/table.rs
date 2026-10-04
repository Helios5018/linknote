use super::{Builder, Glyph, tag_style};
use pulldown_cmark::{Alignment, Event, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use std::ops::Range;

struct Cell {
    glyphs: Vec<Glyph>,
    source: Range<usize>,
}

struct SourceRow {
    prefix: Cell,
    cells: Vec<Cell>,
    separators: Vec<usize>,
    suffix: Cell,
}

fn raw_cell(text: &str, source: Range<usize>) -> Cell {
    let mut builder = Builder::new(usize::MAX / 2);
    builder.raw(&text[source.clone()], source.start, Style::default());
    Cell {
        glyphs: builder.current.glyphs,
        source,
    }
}

fn source_row(text: &str, range: Range<usize>) -> SourceRow {
    let line = &text[range.clone()];
    let mut escaped = false;
    let mut pipes = vec![];
    for (i, ch) in line.char_indices() {
        if ch == '|' && !escaped {
            pipes.push(range.start + i);
        }
        escaped = ch == '\\' && !escaped;
    }
    let leading = pipes
        .first()
        .copied()
        .filter(|&p| text[range.start..p].trim().is_empty());
    let trailing = pipes
        .last()
        .copied()
        .filter(|&p| Some(p) != leading && text[p + 1..range.end].trim().is_empty());
    let start = leading.map_or(range.start, |p| p + 1);
    let end = trailing.unwrap_or(range.end);
    let separators: Vec<_> = pipes
        .into_iter()
        .filter(|&p| Some(p) != leading && Some(p) != trailing)
        .collect();
    let mut from = start;
    let mut cells = vec![];
    for &pipe in &separators {
        cells.push(raw_cell(text, from..pipe));
        from = pipe + 1;
    }
    cells.push(raw_cell(text, from..end));
    SourceRow {
        prefix: raw_cell(text, range.start..start),
        cells,
        separators,
        suffix: raw_cell(text, end..range.end),
    }
}

impl Cell {
    fn lines(&self, width: usize) -> Vec<Vec<&Glyph>> {
        let mut lines = vec![vec![]];
        let mut used = 0;
        for glyph in &self.glyphs {
            if used > 0 && used + glyph.width > width {
                lines.push(vec![]);
                used = 0;
            }
            lines.last_mut().unwrap().push(glyph);
            used += glyph.width;
        }
        lines
    }
}

impl Builder {
    /// Align the original Markdown, including delimiters, without formatting the file.
    /// Real glyphs occur once; padding and continuation pipes only affect the view.
    pub(super) fn table_source(&mut self, text: &str, range: Range<usize>) -> bool {
        let mut start = range.start;
        let mut rows = vec![];
        for line in text[range.clone()].split('\n') {
            rows.push(source_row(text, start..start + line.len()));
            start += line.len() + 1;
        }
        let columns = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
        if columns == 0 {
            return false;
        }
        for row in &mut rows {
            while row.cells.len() < columns {
                row.cells.push(raw_cell(
                    text,
                    row.suffix.source.start..row.suffix.source.start,
                ));
            }
        }
        let prefix = rows
            .iter()
            .map(|r| r.prefix.glyphs.iter().map(|g| g.width).sum::<usize>())
            .max()
            .unwrap_or(0)
            .max(1);
        let suffix = rows
            .iter()
            .map(|r| r.suffix.glyphs.iter().map(|g| g.width).sum::<usize>())
            .max()
            .unwrap_or(0)
            .max(1);
        let minimum: Vec<_> = (0..columns)
            .map(|c| {
                rows.iter()
                    .flat_map(|r| &r.cells[c].glyphs)
                    .map(|g| g.width)
                    .max()
                    .unwrap_or(2)
                    .max(2)
            })
            .collect();
        let overhead = prefix + suffix + columns - 1 + columns * 2;
        // Leave a cell at the right edge for the newline insertion point.
        if overhead + minimum.iter().sum::<usize>() >= self.width {
            return false;
        }
        let mut widths: Vec<_> = (0..columns)
            .map(|c| {
                rows.iter()
                    .map(|r| r.cells[c].glyphs.iter().map(|g| g.width).sum::<usize>())
                    .max()
                    .unwrap_or(0)
                    .max(minimum[c])
                    .min(self.width)
            })
            .collect();
        let budget = self.width - overhead - 1;
        while widths.iter().sum::<usize>() > budget {
            let c = (0..columns)
                .filter(|&i| widths[i] > minimum[i])
                .max_by_key(|&i| widths[i])
                .unwrap();
            widths[c] -= 1;
        }
        let dim = Style::default().fg(Color::Rgb(77, 96, 111));
        self.source_rule(&widths, prefix, suffix, ['╭', '┬', '╮'], range.start, dim);
        for (index, row) in rows.iter().enumerate() {
            let wrapped: Vec<_> = row
                .cells
                .iter()
                .zip(&widths)
                .map(|(c, &w)| c.lines(w))
                .collect();
            let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
            for line in 0..height {
                self.newline(row.prefix.source.start);
                let mut used = 0;
                for g in &row.prefix.glyphs {
                    self.glyph(
                        &g.text,
                        if line == 0 {
                            g.source.clone()
                        } else {
                            g.source.start..g.source.start
                        },
                        dim,
                    );
                    used += g.width;
                }
                self.synthetic(&" ".repeat(prefix - used), row.cells[0].source.start, dim);
                for (col, cell) in row.cells.iter().enumerate() {
                    let glyphs = wrapped[col].get(line);
                    let begin = glyphs
                        .and_then(|gs| gs.first())
                        .map_or(cell.source.end, |g| g.source.start);
                    self.synthetic(" ", begin, dim);
                    let used = glyphs.map_or(0, |gs| gs.iter().map(|g| g.width).sum());
                    if let Some(gs) = glyphs {
                        for g in gs {
                            self.glyph(&g.text, g.source.clone(), g.style);
                        }
                    }
                    let at = glyphs
                        .and_then(|gs| gs.last())
                        .map_or(cell.source.end, |g| g.source.end);
                    self.synthetic(&" ".repeat(widths[col] - used + 1), at, dim);
                    if col + 1 < columns {
                        let pipe = row.separators.get(col).copied().unwrap_or(cell.source.end);
                        let actual =
                            row.separators.get(col).is_some() && line + 1 == wrapped[col].len();
                        self.glyph("|", pipe..pipe + usize::from(actual), dim);
                    }
                }
                let used: usize = row.suffix.glyphs.iter().map(|g| g.width).sum();
                for g in &row.suffix.glyphs {
                    self.glyph(
                        &g.text,
                        if line + 1 == height {
                            g.source.clone()
                        } else {
                            g.source.start..g.source.start
                        },
                        dim,
                    );
                }
                self.synthetic(&" ".repeat(suffix - used), row.suffix.source.end, dim);
                self.current.end = row.suffix.source.end;
            }
            let next = rows.get(index + 1);
            let at = next.map_or(row.suffix.source.end, |r| r.prefix.source.start);
            self.newline(at);
            self.source_rule(
                &widths,
                prefix,
                suffix,
                if next.is_some() {
                    ['├', '┼', '┤']
                } else {
                    ['╰', '┴', '╯']
                },
                at,
                dim,
            );
        }
        true
    }

    fn source_rule(
        &mut self,
        widths: &[usize],
        prefix: usize,
        suffix: usize,
        joints: [char; 3],
        at: usize,
        style: Style,
    ) {
        self.synthetic(&" ".repeat(prefix - 1), at, style);
        self.table_rule(widths, joints, at, style);
        self.synthetic(&" ".repeat(suffix - 1), at, style);
        self.current.separator = true;
    }

    /// Consume the table's events without re-parsing cells, preserving reference links,
    /// escaped pipes, styles and the original byte offsets used for click-to-edit.
    pub(super) fn table<'a>(
        &mut self,
        text: &str,
        source: Range<usize>,
        alignments: &[Alignment],
        events: &mut impl Iterator<Item = (Event<'a>, Range<usize>)>,
        offset: usize,
        base_style: Style,
    ) {
        let mut rows: Vec<Vec<Cell>> = vec![];
        let mut cell: Option<(Builder, Range<usize>)> = None;
        let mut styles = vec![base_style];
        for (event, local) in events.by_ref() {
            let range = offset + local.start..offset + local.end;
            let style = *styles.last().unwrap();
            match event {
                Event::End(TagEnd::Table) => break,
                Event::Start(Tag::TableHead | Tag::TableRow) => rows.push(vec![]),
                Event::Start(Tag::TableCell) => {
                    cell = Some((Builder::new(usize::MAX / 2), range));
                    styles.truncate(1);
                }
                Event::End(TagEnd::TableCell) => {
                    if let Some((builder, source)) = cell.take() {
                        rows.last_mut().unwrap().push(Cell {
                            glyphs: builder.current.glyphs,
                            source,
                        });
                    }
                }
                Event::End(TagEnd::TableHead | TagEnd::TableRow) => {}
                Event::Start(tag) => {
                    if let (Tag::Image { .. }, Some((b, _))) = (&tag, &mut cell) {
                        b.synthetic("[图片: ", range.start, style);
                    }
                    styles.push(tag_style(style, &tag));
                }
                Event::End(tag) => {
                    if tag == TagEnd::Image
                        && let Some((b, _)) = &mut cell
                    {
                        b.synthetic("]", range.end, style);
                    }
                    if styles.len() > 1 {
                        styles.pop();
                    }
                }
                Event::Text(value) | Event::Html(value) | Event::InlineHtml(value) => {
                    if let Some((b, _)) = &mut cell {
                        b.mapped(&value, text, range, style);
                    }
                }
                Event::Code(value) => {
                    if let Some((b, _)) = &mut cell {
                        b.mapped(&value, text, range, style.fg(Color::Rgb(225, 191, 136)));
                    }
                }
                Event::SoftBreak | Event::HardBreak => {
                    if let Some((b, _)) = &mut cell {
                        b.synthetic(" ", range.start, style);
                    }
                }
                _ => {
                    if let Some((b, _)) = &mut cell {
                        b.raw(&text[range.clone()], range.start, style);
                    }
                }
            }
        }
        let columns = alignments.len();
        if rows.is_empty() || columns == 0 {
            return;
        }
        // Missing cells are displayed as empty, without fabricating source bytes.
        for row in &mut rows {
            while row.len() < columns {
                row.push(Cell {
                    glyphs: vec![],
                    source: source.start..source.start,
                });
            }
        }
        let minimum: Vec<usize> = (0..columns)
            .map(|col| {
                rows.iter()
                    .flat_map(|r| r[col].glyphs.iter())
                    .map(|g| g.width)
                    .max()
                    .unwrap_or(2)
                    .max(2)
            })
            .collect();
        let overhead = columns * 3 + 1; // two cell paddings and a shared vertical border
        if minimum.iter().sum::<usize>() + overhead > self.width {
            self.table_stacked(&rows, base_style);
            return;
        }
        let mut widths: Vec<usize> = (0..columns)
            .map(|col| {
                rows.iter()
                    .map(|r| r[col].glyphs.iter().map(|g| g.width).sum::<usize>())
                    .max()
                    .unwrap_or(0)
                    .max(minimum[col])
                    .min(self.width)
            })
            .collect();
        let budget = self.width - overhead;
        while widths.iter().sum::<usize>() > budget {
            let col = (0..columns)
                .filter(|&i| widths[i] > minimum[i])
                .max_by_key(|&i| widths[i])
                .unwrap();
            widths[col] -= 1;
        }
        let border = base_style.fg(Color::Rgb(77, 96, 111));
        self.table_rule(&widths, ['╭', '┬', '╮'], source.start, border);
        for (index, row) in rows.iter().enumerate() {
            let wrapped: Vec<_> = row
                .iter()
                .zip(&widths)
                .map(|(cell, &width)| cell.lines(width))
                .collect();
            let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
            let header = index == 0;
            let cell_style = if header {
                base_style
                    .fg(Color::Rgb(151, 212, 185))
                    .add_modifier(Modifier::BOLD)
            } else {
                base_style
            };
            for line in 0..height {
                self.newline(row[0].source.start);
                self.synthetic("│", row[0].source.start, border);
                for (col, cell) in row.iter().enumerate().take(columns) {
                    let glyphs = wrapped[col].get(line);
                    let used = glyphs.map_or(0, |line| line.iter().map(|g| g.width).sum());
                    let spare = widths[col] - used;
                    let left = match alignments[col] {
                        Alignment::Center => spare / 2,
                        Alignment::Right => spare,
                        _ => 0,
                    };
                    let at_start = glyphs
                        .and_then(|g| g.first())
                        .map_or(cell.source.start, |g| g.source.start);
                    let at_end = glyphs
                        .and_then(|g| g.last())
                        .map_or(cell.source.end, |g| g.source.end);
                    self.synthetic(&" ".repeat(left + 1), at_start, cell_style);
                    if let Some(glyphs) = glyphs {
                        for g in glyphs {
                            self.glyph(
                                &g.text,
                                g.source.clone(),
                                if header {
                                    g.style.patch(cell_style)
                                } else {
                                    g.style
                                },
                            );
                        }
                    }
                    self.synthetic(&" ".repeat(spare - left + 1), at_end, cell_style);
                    self.synthetic("│", at_end, border);
                }
            }
            if index + 1 < rows.len() {
                let at = row.last().unwrap().source.end;
                self.newline(at);
                self.table_rule(&widths, ['├', '┼', '┤'], at, border);
            }
        }
        // Anchor the bottom border inside the table, including clicks near EOF.
        let last = rows.last().unwrap().last().unwrap().source.end;
        self.newline(last);
        self.table_rule(&widths, ['╰', '┴', '╯'], last, border);
    }

    fn table_rule(&mut self, widths: &[usize], joints: [char; 3], at: usize, style: Style) {
        let parts: Vec<_> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
        self.synthetic(
            &format!(
                "{}{}{}",
                joints[0],
                parts.join(&joints[1].to_string()),
                joints[2]
            ),
            at,
            style,
        );
    }

    /// If even two display cells per column cannot fit, show each record vertically.
    /// This keeps all columns readable instead of clipping data off the right edge.
    fn table_stacked(&mut self, rows: &[Vec<Cell>], style: Style) {
        let header = &rows[0];
        for (index, row) in rows.iter().enumerate().skip(1) {
            if index > 1 {
                self.ensure_break(row[0].source.start);
                self.synthetic(
                    &"─".repeat(self.width),
                    row[0].source.start,
                    style.fg(Color::DarkGray),
                );
            }
            for (col, cell) in row.iter().enumerate() {
                self.ensure_break(cell.source.start);
                for g in &header[col].glyphs {
                    self.glyph(
                        &g.text,
                        g.source.clone(),
                        g.style
                            .fg(Color::Rgb(151, 212, 185))
                            .add_modifier(Modifier::BOLD),
                    );
                }
                self.synthetic(": ", cell.source.start, style.fg(Color::DarkGray));
                for g in &cell.glyphs {
                    self.glyph(&g.text, g.source.clone(), g.style);
                }
            }
        }
        if rows.len() == 1 {
            for cell in header {
                self.ensure_break(cell.source.start);
                for g in &cell.glyphs {
                    self.glyph(
                        &g.text,
                        g.source.clone(),
                        g.style.add_modifier(Modifier::BOLD),
                    );
                }
            }
        }
    }
}
