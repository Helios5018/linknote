use linknote::{
    document::Document,
    layout::{self, Layout},
};
use ratatui::style::{Color, Modifier};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

fn rendered(source: &str, width: usize) -> Layout {
    let mut doc = Document::new(source.into());
    doc.set_cursor(source.len(), false);
    Layout::new(&doc, width)
}

#[test]
fn tables_align_cjk_and_emoji_and_preserve_inline_styles() {
    let text = "| 操作 | 快捷键 |\n| :--- | ---: |\n| **新建笔记** | `Ctrl+N` |\n| 帮助 👨‍👩‍👧‍👦 | F1 |\n\n结尾\n";
    let l = rendered(text, 60);
    println!("{}", l.plain());
    let table: Vec<_> = l
        .rows
        .iter()
        .filter(|r| {
            r.glyphs
                .first()
                .is_some_and(|g| ["╭", "│", "├", "╰"].contains(&g.text.as_str()))
        })
        .collect();
    assert_eq!(table.len(), 7);
    let widths: Vec<_> = table.iter().map(|r| r.width).collect();
    assert!(widths.iter().all(|&w| w == widths[0]));
    assert!(l.plain().contains("╭──────────┬────────╮"));
    assert!(l.plain().contains("│ 帮助 👨‍👩‍👧‍👦  │     F1 │"));
    assert!(!l.plain().contains("**"));
    assert!(
        l.rows
            .iter()
            .flat_map(|r| &r.glyphs)
            .any(|g| g.text == "新" && g.style.add_modifier.contains(Modifier::BOLD))
    );
}

#[test]
fn narrow_tables_wrap_cells_without_losing_source_mapping() {
    let text = "| 中文列 | Value |\n| --- | --- |\n| 很长的中文内容和emoji🌿 | abcdefghijklmnop |\n| 短项 | tail |\n\nend\n";
    for width in 14..32 {
        let l = rendered(text, width);
        assert!(l.rows.iter().all(|r| r.width <= width), "{}", l.plain());
        let plain = l.plain();
        let lines: Vec<_> = plain.lines().collect();
        let dividers: Vec<_> = lines
            .iter()
            .enumerate()
            .filter_map(|(i, line)| line.starts_with('├').then_some(i))
            .collect();
        assert_eq!(
            dividers.len(),
            2,
            "header and each data record must be separated\n{plain}"
        );
        let wrapped_record = &lines[dividers[0] + 1..dividers[1]];
        assert!(
            wrapped_record.len() > 1,
            "the long record must wrap\n{plain}"
        );
        assert!(
            wrapped_record
                .iter()
                .all(|line| line.starts_with('│') && line.ends_with('│')),
            "continuations stay within one cell\n{plain}"
        );
        for (y, row) in l.rows.iter().enumerate() {
            let mut x = 0;
            for glyph in &row.glyphs {
                if !glyph.source.is_empty() {
                    assert_eq!(l.hit(y, x), glyph.source.start);
                    let mut d = Document::new(text.into());
                    d.set_cursor(l.hit(y, x), false);
                    let expanded = Layout::new(&d, width);
                    let (ey, ex) = expanded.position(d.cursor);
                    assert_eq!(expanded.hit(ey, ex), d.cursor);
                }
                x += glyph.width;
            }
        }
        let start = text.find("很长").unwrap();
        let end = start + "很长的中文内容和emoji🌿".len();
        let displayed: String = l
            .rows
            .iter()
            .flat_map(|r| &r.glyphs)
            .filter(|g| !g.source.is_empty() && g.source.start >= start && g.source.end <= end)
            .map(|g| g.text.as_str())
            .collect();
        assert!(displayed.contains("很长的中文内容和emoji🌿"));
    }
}

#[test]
fn too_many_columns_stack_readable_records_and_empty_cells_work() {
    let text = "| A | B | C | D |\n|---|---|---|---|\n| one | | 三 | four |\n\nend\n";
    let l = rendered(text, 16);
    assert!(l.plain().contains("A: one"));
    assert!(l.plain().contains("C: 三"));
    assert!(l.plain().contains("D: four"));
    assert!(l.rows.iter().all(|r| r.width <= 16));
}

#[test]
fn escaped_pipe_reference_link_alignment_and_source_are_preserved() {
    let text = "| A | 中 | C |\n|:---|:---:|---:|\n| a\\|b | **c** | [link][r] |\n| longer | 中文 | x |\n\n[r]: https://example.com\n\nend\n";
    let l = rendered(text, 60);
    println!("{}", l.plain());
    assert!(l.plain().contains("a|b"));
    // The top-level parser's reference context must survive table rendering.
    assert!(l.plain().contains("link"));
    assert!(!l.plain().contains("[link][r]"));
    let rows: Vec<_> = l
        .plain()
        .lines()
        .filter(|line| line.starts_with('│'))
        .map(str::to_owned)
        .collect();
    assert!(rows.iter().any(|line| line.contains("│  c   │")));
    assert!(
        rows.iter()
            .map(|line| UnicodeWidthStr::width(line.as_str()))
            .all(|w| w == UnicodeWidthStr::width(rows[0].as_str()))
    );
    let mut doc = Document::new(text.into());
    doc.set_cursor(text.find("a\\|b").unwrap(), false);
    let editing = Layout::new(&doc, 60).plain();
    assert!(editing.contains('╭') && editing.contains('├'));
    assert!(editing.contains("a\\|b") && editing.contains("[link][r]"));
    assert_eq!(doc.text, text);
}

#[test]
fn lists_have_distinct_levels_and_hierarchical_numbers() {
    let text = "- 一级\n  - 二级\n    - 三级\n  - 同级\n- 返回一级\n\n1. 第一\n   1. 子项\n      1. 孙项\n   2. 第二子项\n2. 第二\n\nend\n";
    let l = rendered(text, 60);
    println!("{}", l.plain());
    for expected in [
        "• 一级",
        "  ◦ 二级",
        "    ▪ 三级",
        "  ◦ 同级",
        "• 返回一级",
        "1. 第一",
        "  1.1. 子项",
        "    1.1.1. 孙项",
        "  1.2. 第二子项",
        "2. 第二",
    ] {
        assert!(
            l.plain().lines().any(|line| line == expected),
            "missing {expected:?}\n{}",
            l.plain()
        );
    }
}

#[test]
fn wrapped_list_continuations_keep_their_indentation() {
    let text =
        "- parent\n  - 这是一段足够长的中文列表内容需要换行并保持缩进\n    显式续行\n\nend\n";
    let l = rendered(text, 20);
    let plain = l.plain();
    let lines: Vec<_> = plain.lines().collect();
    let start = lines.iter().position(|s| s.starts_with("  ◦ ")).unwrap();
    assert!(lines[start + 1].starts_with("    "), "{plain}");
    assert!(lines.contains(&"    显式续行"), "{plain}");
    assert!(l.rows.iter().all(|r| r.width <= 20));
}

#[test]
fn source_tables_align_while_editing_without_inserting_file_padding() {
    let source = "| 名称 | 值 |\n| --- | --- |\n| 中文🌿 | **long** |\n| x | y |\n\nend\n";
    let mut doc = Document::new(source.into());
    doc.set_cursor(source.find("中文").unwrap(), false);
    let layout = Layout::new(&doc, 70);
    println!("{}", layout.plain());
    let pipes: Vec<Vec<usize>> = layout
        .rows
        .iter()
        .filter(|row| row.glyphs.first().is_some_and(|g| g.text == "|"))
        .map(|row| {
            let mut x = 0;
            row.glyphs
                .iter()
                .filter_map(|g| {
                    let at = x;
                    x += g.width;
                    (g.text == "|").then_some(at)
                })
                .collect()
        })
        .collect();
    assert_eq!(pipes.len(), 4);
    assert!(pipes.iter().all(|p| p == &pipes[0]));
    assert!(layout.plain().contains("**long**"));
    assert_eq!(layout.rows.iter().filter(|r| r.separator).count(), 5);
    let from = source.find("中文").unwrap();
    let (y, x) = layout.position(from);
    let next = layout.vertical_hit(y, y + 1, x);
    assert!(
        next >= source.find("| x").unwrap(),
        "Down skips the rule to the next source record"
    );
    let (ny, nx) = layout.position(next);
    let previous = layout.vertical_hit(ny, ny - 1, nx);
    assert_eq!(previous, from);
    assert_eq!(doc.text, source);
    doc.insert("新增");
    assert_eq!(doc.text, source.replacen("中文", "新增中文", 1));
    doc.undo();
    assert_eq!(doc.text, source);
}

#[test]
fn chinese_punctuation_bold_is_visible_but_code_and_active_source_stay_literal() {
    let text = "**采取行动（Action）：**工作之外仍需保持学习。\n\n**调整信念（Belief）：**传统信念下。\n\n`**代码：**正文`\n\n```md\n**代码块：**正文\n```\n\nend\n";
    let l = rendered(text, 70);
    assert!(!l.plain().contains("**采取行动"), "{}", l.plain());
    assert!(!l.plain().contains("**调整信念"));
    assert!(
        l.rows
            .iter()
            .flat_map(|r| &r.glyphs)
            .any(|g| g.text == "采" && g.style.add_modifier.contains(Modifier::BOLD))
    );
    assert!(
        l.rows
            .iter()
            .flat_map(|r| &r.glyphs)
            .filter(|g| g.style.add_modifier.contains(Modifier::BOLD))
            .all(|g| g.style.fg == Some(Color::Rgb(242, 239, 226))),
        "strong text keeps its requested highlight color as well as bold"
    );
    assert!(l.plain().contains("**代码：**正文"));
    assert!(l.plain().contains("**代码块：**正文"));
    let mut d = Document::new(text.into());
    d.set_cursor(2, false);
    assert!(
        Layout::new(&d, 70)
            .plain()
            .contains("**采取行动（Action）：**")
    );
    assert_eq!(d.text, text);
}

#[test]
fn wrapped_source_table_cursor_maps_every_source_boundary_and_keeps_all_bytes() {
    for source in [
        "| 中文长列 | Value |\n| --- | --- |\n| 很长的中文👨‍👩‍👧‍👦é和内容 | abcdefghijklmnop |\n| | 空 |\n\n",
        "A | B\n--- | ---\n转义\\|竖线 | x\n短 | 内容很长的右侧\n\n",
    ] {
        for width in [18, 28, 60] {
            for offset in source
                .grapheme_indices(true)
                .map(|(i, _)| i)
                .chain(std::iter::once(source.len()))
            {
                let mut doc = Document::new(source.into());
                doc.set_cursor(offset, false);
                let l = Layout::new(&doc, width);
                let (y, x) = l.position(doc.cursor);
                assert_eq!(
                    l.hit(y, x),
                    doc.cursor,
                    "width={width} offset={offset} ({y},{x})\n{}",
                    l.plain()
                );
                assert!(l.rows.iter().all(|r| r.width <= width));
                assert_eq!(doc.text, source);
            }
            let mut doc = Document::new(source.into());
            doc.set_cursor(0, false);
            let l = Layout::new(&doc, width);
            let mut glyphs: Vec<_> = l
                .rows
                .iter()
                .flat_map(|r| &r.glyphs)
                .filter(|g| !g.source.is_empty())
                .collect();
            glyphs.sort_by_key(|g| g.source.start);
            assert_eq!(
                glyphs.iter().map(|g| g.text.as_str()).collect::<String>(),
                source.replace('\n', "")
            );
        }
    }
}

/// The notes in `docs/demo` double as a corpus of unusual Markdown: every block, active
/// or not, must lay out at narrow and wide widths with glyphs mapped inside the source.
#[test]
fn demo_notes_lay_out_with_each_block_active() {
    fn notes(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                notes(&path, out);
            } else if path.extension().is_some_and(|e| e == "md") {
                out.push(path);
            }
        }
    }
    let mut paths = vec![];
    notes(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/demo"),
        &mut paths,
    );
    assert!(paths.len() > 40);
    for path in paths {
        let text = std::fs::read_to_string(&path).unwrap();
        let mut doc = Document::new(text.clone());
        // Activating every block of the long note would only repeat the same shapes.
        let cursors: Vec<_> = layout::blocks(&text)
            .into_iter()
            .map(|b| b.start)
            .take(if text.len() > 32 * 1024 {
                4
            } else {
                usize::MAX
            })
            .chain([text.len()])
            .collect();
        for width in [6, 24, 80, 160] {
            for &cursor in &cursors {
                doc.set_cursor(cursor, false);
                let l = Layout::new(&doc, width);
                for g in l.rows.iter().flat_map(|r| &r.glyphs) {
                    assert!(
                        g.source.end <= text.len()
                            && text.is_char_boundary(g.source.start)
                            && text.is_char_boundary(g.source.end),
                        "{}: width {width}, cursor {cursor}, glyph {:?}",
                        path.display(),
                        g.text
                    );
                }
            }
        }
    }
}
