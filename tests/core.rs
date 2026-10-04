use linknote::{
    document::Document,
    layout::{Layout, blocks},
    storage::Note,
    tree,
};
use proptest::prelude::*;
use std::{collections::HashSet, fs};
use unicode_segmentation::UnicodeSegmentation;

#[test]
fn hybrid_blocks_preserve_source_and_reveal_only_active_block() {
    let source = "# 标题\n\n这是 **加粗** 和 [链接](https://example.com)。\n\n```rust\nlet x = 1;\n```\n\n最后一段\n";
    let mut d = Document::new(source.into());
    d.set_cursor(source.find("这是").unwrap(), false);
    let view = Layout::new(&d, 60);
    println!("blocks: {:?}\n{}", blocks(source), view.plain());
    assert_eq!(view.active.len(), 1);
    assert!(view.plain().contains("**加粗**"));
    assert!(!view.plain().contains("# 标题"));
    assert!(!view.plain().contains("```"));
    assert_eq!(d.text, source);
    d.set_cursor(source.find("let x").unwrap(), false);
    let view = Layout::new(&d, 60);
    assert!(view.plain().contains("```rust"));
    assert!(!view.plain().contains("**加粗**"));
}

#[test]
fn chinese_emoji_wrap_and_hit_roundtrip() {
    let source = "中文👨‍👩‍👧‍👦e\u{301}和更多文字，测试自动换行。";
    for offset in source
        .grapheme_indices(true)
        .map(|(i, _)| i)
        .chain(std::iter::once(source.len()))
    {
        let mut d = Document::new(source.into());
        d.set_cursor(offset, false);
        for width in 4..20 {
            let view = Layout::new(&d, width);
            let (y, x) = view.position(offset);
            assert_eq!(
                view.hit(y, x),
                offset,
                "width={width}, offset={offset}, ({y},{x})"
            );
            assert!(view.rows.iter().all(|r| r.width <= width));
        }
    }
}

#[test]
fn full_width_line_end_has_a_real_insertion_cell() {
    for source in ["abcd\nnext", "中文\n下一段", "abcd"] {
        let mut d = Document::new(source.into());
        let end = source.find('\n').unwrap_or(source.len());
        d.set_cursor(end, false);
        let l = Layout::new(&d, 4);
        let (y, x) = l.position(end);
        assert_eq!(l.hit(y, x), end, "{source:?}");
    }
}

#[test]
fn selection_across_blocks_and_undo_is_one_document() {
    let s = "# 标题\n\n第二段 **内容**\n\n第三段";
    let mut d = Document::new(s.into());
    d.set_cursor(2, false);
    d.set_cursor(s.find("第三").unwrap(), true);
    assert!(Layout::new(&d, 30).active.len() >= 2);
    d.insert("替换\n跨段落");
    d.undo();
    assert_eq!(d.text, s);
    d.redo();
    assert!(d.text.contains("替换\n跨段落"));
}

#[test]
fn deletes_whole_graphemes() {
    let mut d = Document::new("A👨‍👩‍👧‍👦e\u{301}".into());
    d.set_cursor(d.text.len(), false);
    d.backspace();
    assert_eq!(d.text, "A👨‍👩‍👧‍👦");
    d.backspace();
    assert_eq!(d.text, "A");
    d.undo();
    assert_eq!(d.text, "A👨‍👩‍👧‍👦e\u{301}");
}

#[test]
fn atomic_save_conflict_recovery_and_crlf() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.md");
    let recovery = dir.path().join("recovery");
    fs::write(&path, "# 标题\r\n\r\n原文\r\n").unwrap();
    let mut n = Note::open(path.clone(), &recovery).unwrap();
    n.doc.set_cursor(n.doc.text.len(), false);
    n.doc.insert("修改\n");
    n.save().unwrap();
    assert!(fs::read_to_string(&path).unwrap().ends_with("修改\r\n"));
    n.doc.insert("本地");
    n.journal().unwrap();
    let expected = n.doc.text.clone();
    drop(n);
    let mut restarted = Note::open(path.clone(), &recovery).unwrap();
    restarted.recover(restarted.draft().unwrap().unwrap());
    assert_eq!(restarted.doc.text, expected);
    fs::write(&path, "外部修改").unwrap();
    assert!(restarted.save().is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "外部修改");
    let copy = restarted.keep_both().unwrap();
    assert!(fs::read_to_string(copy).unwrap().contains("本地"));
    assert!(!restarted.recovery_path.exists());
}

#[test]
fn clean_external_reload_and_undo_can_recover_previous_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.md");
    fs::write(&path, "before").unwrap();
    let mut n = Note::open(path.clone(), dir.path()).unwrap();
    fs::write(&path, "after").unwrap();
    assert!(n.check_external().unwrap());
    assert_eq!(n.doc.text, "after");
    assert!(!n.dirty());
    n.doc.undo();
    assert!(n.dirty());
    assert_eq!(n.doc.text, "before");
}

#[test]
fn tree_filters_files_preserves_empty_folders_and_nested_notes() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("空文件夹")).unwrap();
    fs::create_dir(dir.path().join("nested")).unwrap();
    fs::write(dir.path().join("nested/A.MD"), "").unwrap();
    fs::write(dir.path().join("ignore.txt"), "").unwrap();
    let expanded = HashSet::from([dir.path().join("nested")]);
    let entries = tree::scan(dir.path(), &expanded).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(tree::all_notes(dir.path()).unwrap().len(), 1);
    assert!(tree::validated_name("../escape", true).is_err());
    assert_eq!(tree::validated_name("新笔记", true).unwrap(), "新笔记.md");
}

proptest! {
    #[test]
    fn arbitrary_unicode_layout_never_corrupts_source(s in ".{0,150}", width in 4usize..90) {
        let mut d = Document::new(s.clone());
        for offset in s.grapheme_indices(true).map(|(i,_)| i).chain(std::iter::once(s.len())) {
            d.set_cursor(offset, false);
            let l = Layout::new(&d, width);
            let (y,x) = l.position(d.cursor);
            let hit = l.hit(y,x);
            prop_assert!(s.is_char_boundary(hit));
            prop_assert_eq!(&d.text, &s);
        }
    }
}

#[test]
fn mixed_endings_are_not_written_until_the_user_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mixed.md");
    let bytes = b"one\r\ntwo\nthree\r\n";
    fs::write(&path, bytes).unwrap();
    let mut n = Note::open(path.clone(), dir.path()).unwrap();
    assert!(!n.dirty());
    n.save().unwrap();
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn undo_to_saved_content_clears_stale_crash_draft() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.md");
    fs::write(&path, "original").unwrap();
    let mut n = Note::open(path, dir.path()).unwrap();
    n.doc.insert("discarded");
    n.journal().unwrap();
    assert!(n.recovery_path.exists());
    n.doc.undo();
    n.journal().unwrap();
    assert!(!n.recovery_path.exists());
}

#[test]
fn concurrent_windows_cannot_share_a_recovery_draft() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.md");
    fs::write(&path, "text").unwrap();
    let first = Note::open(path.clone(), dir.path()).unwrap();
    assert!(Note::open(path.clone(), dir.path()).is_err());
    drop(first);
    assert!(Note::open(path, dir.path()).is_ok());
}

#[test]
fn missing_file_does_not_destroy_local_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.md");
    fs::write(&path, "original").unwrap();
    let mut n = Note::open(path.clone(), dir.path()).unwrap();
    n.doc.insert("local");
    fs::remove_file(&path).unwrap();
    assert!(n.save().is_err());
    assert!(!path.exists());
    assert_eq!(n.doc.text, "localoriginal");
    assert!(n.recovery_path.exists());
}

#[cfg(unix)]
#[test]
fn read_only_save_failure_retains_original_and_recovery() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.md");
    fs::write(&path, "original").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    let mut n = Note::open(path.clone(), dir.path()).unwrap();
    n.doc.insert("local");
    assert!(n.save().is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "original");
    assert!(n.recovery_path.exists());
    assert_eq!(n.doc.text, "localoriginal");
}

#[test]
fn list_continuation_and_empty_marker_exit() {
    let mut d = Document::new("- [x] 完成".into());
    d.set_cursor(d.text.len(), false);
    d.newline();
    assert_eq!(d.text, "- [x] 完成\n- [ ] ");
    d.newline();
    assert_eq!(d.text, "- [x] 完成\n");
    let mut d = Document::new("9. item".into());
    d.set_cursor(d.text.len(), false);
    d.newline();
    assert_eq!(d.text, "9. item\n10. ");
}

#[test]
#[ignore = "release-mode measurement: cargo test --release layout_timing -- --ignored --nocapture"]
fn layout_timing() {
    let source = "# 标题\n\n中文与 **粗体**、[链接](https://example.com)，这是用于测量的段落。\n\n"
        .repeat(800);
    let mut d = Document::new(source);
    let start = std::time::Instant::now();
    for i in 0..30 {
        d.set_cursor(i * 93, false);
        std::hint::black_box(Layout::new(&d, 90));
    }
    println!(
        "{} bytes, 30 layouts: {:?}, mean {:?}",
        d.text.len(),
        start.elapsed(),
        start.elapsed() / 30
    );
}

proptest! {
    #[test]
    fn local_grapheme_boundaries_match_full_segmentation(s in ".{0,60}", at in 0usize..250) {
        let d = Document::new(s.clone());
        let at = at % (s.len() + 1);
        let expected = if at >= s.len() {
            s.len()
        } else {
            s.grapheme_indices(true).map(|(i, _)| i).take_while(|i| *i <= at).last().unwrap_or(0)
        };
        prop_assert_eq!(d.boundary(at), expected);
        let b = d.boundary(at);
        let next = s[b..].graphemes(true).next().map_or(b, |g| b + g.len());
        prop_assert_eq!(d.next(b), next);
        let previous = s[..b].grapheme_indices(true).next_back().map_or(0, |(i, _)| i);
        prop_assert_eq!(d.previous(b), previous);
    }
}

#[test]
fn word_movement_stops_inside_chinese_text() {
    let s = "中文段落，第二句 English words";
    let mut d = Document::new(s.into());
    let mut stops = vec![];
    while d.cursor < s.len() {
        d.horizontal(true, false, true);
        stops.push(&s[..d.cursor]);
    }
    assert_eq!(
        stops,
        [
            "中文段落",
            "中文段落，",
            "中文段落，第二句",
            "中文段落，第二句 English",
            "中文段落，第二句 English words"
        ]
    );
    d.horizontal(false, false, true);
    assert_eq!(&s[d.cursor..], "words");
    d.delete_word_back();
    assert_eq!(d.text, "中文段落，第二句 words");
}

#[test]
fn tab_nests_list_items_under_the_previous_sibling_and_shift_tab_outdents() {
    let mut d = Document::new("- 一\n- 二\n- 三".into());
    d.set_cursor(d.text.find("二").unwrap(), false);
    assert!(d.indent(false));
    assert_eq!(d.text, "- 一\n  - 二\n- 三");
    assert_eq!(
        &d.text[d.cursor..d.cursor + 3],
        "二",
        "cursor stays on the same text"
    );
    assert!(d.indent(true));
    assert_eq!(d.text, "- 一\n- 二\n- 三");
    let mut d = Document::new("1. 一\n2. 二".into());
    d.set_cursor(d.text.len(), false);
    d.indent(false);
    assert_eq!(d.text, "1. 一\n   2. 二");
    // Undo restores the previous indentation in one step.
    d.undo();
    assert_eq!(d.text, "1. 一\n2. 二");
    // Plain text: Tab inserts spaces instead.
    let mut d = Document::new("正文".into());
    assert!(!d.indent(false));
    // A selection indents every touched line.
    let mut d = Document::new("a\nb\n".into());
    d.select_all();
    d.indent(false);
    assert_eq!(d.text, "    a\n    b\n");
}

#[test]
fn enter_on_an_empty_nested_item_moves_it_up_one_level() {
    let mut d = Document::new("- 一\n  - ".into());
    d.set_cursor(d.text.len(), false);
    d.newline();
    assert_eq!(d.text, "- 一\n- ");
    d.newline();
    assert_eq!(d.text, "- 一\n");
    let mut d = Document::new("3) 项".into());
    d.set_cursor(d.text.len(), false);
    d.newline();
    assert_eq!(d.text, "3) 项\n4) ");
}

#[test]
fn undo_history_stores_edits_not_copies_of_large_notes() {
    let big = "段落内容。\n".repeat(400_000); // about 6 MiB
    let mut d = Document::new(big.clone());
    for i in 0..300 {
        d.set_cursor(i * 7, false);
        d.insert("x");
    }
    // Every step is still undoable; full snapshots would have hit the 16 MiB cap.
    for _ in 0..300 {
        d.undo();
    }
    assert_eq!(d.text, big);
    for _ in 0..300 {
        d.redo();
    }
    assert_eq!(d.text.matches('x').count(), 300);
    d.reload("新内容".into());
    d.undo();
    assert_eq!(d.text.matches('x').count(), 300);
}

#[test]
fn drafts_store_a_fingerprint_and_old_drafts_still_recover() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.md");
    let recovery = dir.path().join("recovery");
    let original = "原文".repeat(2000);
    fs::write(&path, &original).unwrap();
    let mut n = Note::open(path.clone(), &recovery).unwrap();
    n.doc.insert("本地");
    n.journal().unwrap();
    let draft = fs::read(&n.recovery_path).unwrap();
    assert!(
        draft.len() < original.len() + 200,
        "draft holds the text once ({} bytes)",
        draft.len()
    );
    let draft_path = n.recovery_path.clone();
    let lock = draft_path.with_extension("lock");
    drop(n);
    assert!(lock.exists(), "lock file stays while a draft exists");

    // A 0.1.0 draft stored the base file as a byte array.
    let mut legacy: serde_json::Value = serde_json::from_slice(&draft).unwrap();
    legacy.as_object_mut().unwrap().remove("base_fingerprint");
    legacy["base"] = serde_json::to_value(original.as_bytes()).unwrap();
    fs::write(&draft_path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let mut n = Note::open(path.clone(), &recovery).unwrap();
    n.recover(n.draft().unwrap().unwrap());
    assert!(!n.conflict);
    n.save().unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("本地{original}")
    );
    drop(n);
    assert!(
        !lock.exists(),
        "lock file is removed once no draft needs it"
    );

    // A draft whose base no longer matches the disk is recovered as a conflict.
    let mut n = Note::open(path.clone(), &recovery).unwrap();
    n.doc.insert("再改");
    n.journal().unwrap();
    drop(n);
    fs::write(&path, "外部修改").unwrap();
    let mut n = Note::open(path.clone(), &recovery).unwrap();
    n.recover(n.draft().unwrap().unwrap());
    assert!(n.conflict);
    assert!(n.save().is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "外部修改");
    n.use_disk().unwrap();
    assert_eq!(n.doc.text, "外部修改");
    n.doc.undo();
    assert!(n.doc.text.starts_with("再改"));
    n.overwrite_disk().unwrap();
    assert!(fs::read_to_string(&path).unwrap().starts_with("再改"));
}

#[test]
fn tree_hides_dot_entries_node_modules_and_ignore_patterns() {
    let dir = tempfile::tempdir().unwrap();
    for d in [".obsidian", "node_modules", "笔记", "build"] {
        fs::create_dir(dir.path().join(d)).unwrap();
    }
    for f in [
        ".hidden.md",
        "draft-1.md",
        "keep.md",
        "笔记/a.md",
        ".obsidian/x.md",
    ] {
        fs::write(dir.path().join(f), "").unwrap();
    }
    fs::write(
        dir.path().join(".linknoteignore"),
        "# comment\ndraft-*\nbuild/\n",
    )
    .unwrap();
    let names: Vec<_> = tree::scan(dir.path(), &HashSet::new())
        .unwrap()
        .iter()
        .map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["笔记", "keep.md"]);
    assert_eq!(tree::all_notes(dir.path()).unwrap().len(), 2);
    assert!(tree::validated_name(".secret", true).is_err());
}

#[test]
fn file_briefly_missing_and_restored_unchanged_is_not_a_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.md");
    let away = dir.path().join("away");
    fs::write(&path, "original").unwrap();
    let mut n = Note::open(path.clone(), &dir.path().join("recovery")).unwrap();
    // A branch switch or sync tool removes the file and puts it back.
    fs::rename(&path, &away).unwrap();
    assert!(n.check_external().is_err());
    assert!(n.conflict);
    n.error = Some("磁盘检查失败".into());
    fs::rename(&away, &path).unwrap();
    assert!(!n.check_external().unwrap());
    assert!(!n.conflict);
    assert!(n.error.is_none());
    n.doc.insert("local ");
    n.edited(false);
    n.save().unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "local original");
}

#[cfg(unix)]
#[test]
fn unreadable_subfolder_does_not_hide_other_notes() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.md"), "").unwrap();
    fs::create_dir(dir.path().join("open")).unwrap();
    fs::write(dir.path().join("open/b.md"), "").unwrap();
    let locked = dir.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let notes = tree::all_notes(dir.path());
    let entries = tree::scan(dir.path(), &HashSet::from([locked.clone()]));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        notes.unwrap(),
        [dir.path().join("open/b.md"), dir.path().join("a.md")]
    );
    assert_eq!(entries.unwrap().len(), 3, "locked folder shown empty");
}
