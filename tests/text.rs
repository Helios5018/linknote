use linknote::{
    document::Document,
    layout::{Layout, link_at},
    links::{self, Target},
    search,
};
use std::fs;

#[test]
fn find_uses_smart_case_and_unicode_case_folding() {
    let text = "Apple apple APPLE Äpfel äpfel";
    assert_eq!(search::find_all(text, "apple").len(), 3);
    assert_eq!(search::find_all(text, "Apple").len(), 1);
    assert_eq!(search::find_all(text, "äpfel").len(), 2);
    assert_eq!(search::find_next(text, "apple", 1), Some(6..11));
    assert_eq!(
        search::find_next(text, "apple", 20),
        Some(0..5),
        "wraps around"
    );
    let (out, n) = search::replace_all("中文 中文 中", "中文", "汉语");
    assert_eq!((out.as_str(), n), ("汉语 汉语 中", 2));
    let hits = search::search_text(
        std::path::Path::new("a.md"),
        "第一行\n这里有关键词。\n",
        "关键词",
        10,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].line, 2);
    assert_eq!(hits[0].snippet, "这里有关键词。");
}

#[test]
fn wiki_links_render_without_brackets_and_never_reach_the_crashing_parser_path() {
    let mut doc = Document::new("见 [[目标笔记|别名]] 和 [[其他]]\n\n末尾".into());
    doc.set_cursor(doc.text.len(), false);
    let plain = Layout::new(&doc, 60).plain();
    assert!(plain.contains("见 别名 和 其他"), "{plain}");
    assert_eq!(
        link_at(&doc.text, doc.text.find("别名").unwrap()),
        Some(("目标笔记".into(), true))
    );
    // pulldown-cmark 0.13.4 panics on this input when its wikilink option is enabled.
    let text = "---\n![[ref]: https://example.com\n图![图](a.png)](a.png)]]---\n[链接][ref]";
    Layout::new(&Document::new(text.into()), 30);
}

#[test]
fn table_rows_with_missing_cells_before_a_list_do_not_crash() {
    let text = "# 标题\n| a | b |\n| - | - |\n|  2 |\n1. 一\n```\ncode\n```\n";
    let mut doc = Document::new(text.into());
    doc.set_cursor(0, false);
    let plain = Layout::new(&doc, 30).plain();
    assert!(plain.contains("1. 一"), "{plain}");
}

#[test]
fn links_resolve_to_notes_headings_wiki_names_and_urls() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("项目/子目录")).unwrap();
    fs::write(root.join("a.md"), "").unwrap();
    fs::write(root.join("项目/b.md"), "").unwrap();
    fs::write(root.join("项目/子目录/深处.md"), "").unwrap();
    fs::write(root.join("项目/图.png"), "").unwrap();
    let note = root.join("项目/b.md");
    assert_eq!(
        links::resolve(&root, &note, "../a.md#第二节", false),
        Target::Note(root.join("a.md"), Some("第二节".into()))
    );
    assert_eq!(
        links::resolve(
            &root,
            &note,
            "%E5%AD%90%E7%9B%AE%E5%BD%95/%E6%B7%B1%E5%A4%84.md",
            false
        ),
        Target::Note(root.join("项目/子目录/深处.md"), None)
    );
    assert_eq!(
        links::resolve(&root, &root.join("a.md"), "深处", true),
        Target::Note(root.join("项目/子目录/深处.md"), None)
    );
    assert_eq!(
        links::resolve(&root, &note, "新想法", true),
        Target::Missing(root.join("项目/新想法.md"))
    );
    assert_eq!(
        links::resolve(&root, &note, "#标题", false),
        Target::Heading("标题".into())
    );
    assert_eq!(
        links::resolve(&root, &note, "https://example.com", false),
        Target::External("https://example.com".into())
    );
    assert_eq!(
        links::resolve(&root, &note, "../../etc/passwd", false),
        Target::Outside
    );
    assert_eq!(
        links::resolve(&root, &note, "子目录", false),
        Target::Folder(root.join("项目/子目录"))
    );
    assert!(matches!(
        links::resolve(&root, &note, "图.png", false),
        Target::External(_)
    ));
    assert_eq!(
        links::url_at("访问 https://example.com/路径。然后", 10),
        Some("https://example.com/".into())
    );
    assert_eq!(links::slug("第二节 Notes!"), "第二节-notes");
}
