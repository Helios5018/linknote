use linknote::{document::Document, layout::Layout};
use proptest::prelude::*;
use std::collections::HashMap;

const PIECES: &[&str] = &[
    "# 标题\n",
    "\n\n",
    "\n",
    "- 项目\n",
    "  - 子项\n",
    "1. 一\n",
    "| a | b |\n| - | - |\n| 1 | 2 |\n",
    "```\ncode\n```\n",
    "```mermaid\nflowchart LR\n A[中文] --> B\n```\n",
    "> ```mermaid\n> flowchart TD\n> A --> B\n> ```\n",
    "~~~mermaid\nflowchart LR\n A --> B\n~~~\n",
    "```\n",
    "> 引用\n",
    "[ref]: https://example.com\n",
    "[链接][ref]",
    "![图](a.png)",
    "**粗**",
    "**采取行动：**继续",
    "中文段落很长很长很长很长很长",
    "[^1]",
    "[^1]: 注\n",
    "---\n",
    "[[维基]]",
    "text",
    " ",
    "|",
    "*",
    "<div>\n",
    "    indented\n",
    "[",
    "]",
    "(",
    ")",
    "!",
    "`",
    "#",
    "-",
    "1. ",
    "\t",
    "  ",
    ">",
    "~~删~~",
    "- [ ] 任务\n",
    "![图](a.png) ![二](a.png)\n",
];

#[derive(Debug, Clone)]
enum Op {
    Insert(usize, usize),
    Delete(usize, usize),
    Cursor(usize),
    Select(usize, usize),
    Width(usize),
    Image(usize),
    Undo,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (any::<usize>(), 0..PIECES.len()).prop_map(|(a, p)| Op::Insert(a, p)),
        (any::<usize>(), 0usize..12).prop_map(|(a, n)| Op::Delete(a, n)),
        any::<usize>().prop_map(Op::Cursor),
        (any::<usize>(), any::<usize>()).prop_map(|(a, b)| Op::Select(a, b)),
        (6usize..60).prop_map(Op::Width),
        (0usize..5).prop_map(Op::Image),
        Just(Op::Undo),
    ]
}

fn assert_same(cached: &Layout, fresh: &Layout, doc: &Document) {
    assert_eq!(cached.active, fresh.active, "{:?}", doc.text);
    assert_eq!(cached.blocks, fresh.blocks, "{:?}", doc.text);
    assert_eq!(
        cached.rows.len(),
        fresh.rows.len(),
        "{:?}\n{}",
        doc.text,
        fresh.plain()
    );
    for (y, (a, b)) in cached.rows.iter().zip(&fresh.rows).enumerate() {
        assert_eq!(a, b, "row {y} of {:?}", doc.text);
    }
    let slots = |l: &Layout| {
        l.images
            .iter()
            .map(|s| (s.url.clone(), s.source.clone(), s.row, s.height))
            .collect::<Vec<_>>()
    };
    assert_eq!(slots(cached), slots(fresh));
    assert_eq!(cached.image_urls, fresh.image_urls);
    for offset in [0, doc.cursor, doc.text.len()] {
        assert_eq!(cached.position(offset), fresh.position(offset));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(
        std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(160)
    ))]
    #[test]
    fn incremental_layout_matches_a_fresh_layout(
        start in proptest::collection::vec(0..PIECES.len(), 0..10),
        ops in proptest::collection::vec(op(), 1..40),
    ) {
        let text: String = start.iter().map(|&i| PIECES[i]).collect();
        let mut doc = Document::new(text);
        let mut width = 30;
        let mut heights = HashMap::new();
        let mut layout = Layout::with_images(&doc, width, &heights);
        for op in ops {
            let len = doc.text.len() + 1;
            match op {
                Op::Insert(at, piece) => {
                    doc.set_cursor(at % len, false);
                    doc.insert(PIECES[piece]);
                }
                Op::Delete(at, n) => {
                    doc.set_cursor(at % len, false);
                    for _ in 0..n {
                        doc.delete();
                    }
                }
                Op::Cursor(at) => doc.set_cursor(at % len, false),
                Op::Select(a, b) => {
                    doc.set_cursor(a % len, false);
                    doc.set_cursor(b % len, true);
                }
                Op::Width(w) => width = w,
                Op::Image(h) => {
                    heights.insert("a.png".to_string(), h);
                    for key in linknote::layout::image_sources(&doc.text) {
                        if linknote::mermaid::source(&key).is_some() {
                            heights.insert(key, h * 10);
                        }
                    }
                }
                Op::Undo => doc.undo(),
            }
            layout = Layout::update(Some(layout), &doc, width, &heights);
            let fresh = Layout::with_images(&doc, width, &heights);
            assert_same(&layout, &fresh, &doc);
        }
    }
}

#[test]
fn moving_between_blocks_reuses_rows_without_changing_output() {
    let source =
        "# 标题\n\n第一段 **粗体**。\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n最后\n".repeat(20);
    let mut doc = Document::new(source);
    let mut layout = Layout::new(&doc, 40);
    for at in [0, 30, 60, 5, doc.text.len(), 12] {
        doc.set_cursor(at, false);
        layout = Layout::update(Some(layout), &doc, 40, &HashMap::new());
        assert_same(&layout, &Layout::new(&doc, 40), &doc);
    }
}
