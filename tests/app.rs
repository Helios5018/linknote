//! Application behaviour driven through the same key, mouse and paste entry points
//! as the terminal, without a PTY.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use linknote::app::{App, Focus, Options};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use unicode_width::UnicodeWidthStr;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    base: PathBuf,
}

fn fixture(files: &[(&str, &str)]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let root = base.join("notes");
    fs::create_dir(&root).unwrap();
    for (name, text) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    Fixture {
        _dir: dir,
        root,
        base,
    }
}

impl Fixture {
    fn options(&self) -> Options {
        Options {
            state_dir: Some(self.base.join("state")),
            config_dir: Some(self.base.join("config")),
            trash_dir: Some(self.base.join("trash")),
            opener: Some("true".into()),
            internal_clipboard: true,
        }
    }
    fn app(&self, initial: Option<&str>) -> App {
        let mut app = App::with_options(
            self.root.clone(),
            initial.map(|n| self.root.join(n)),
            self.options(),
        )
        .unwrap();
        draw(&mut app);
        app
    }
    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

fn key(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}
fn ctrl(app: &mut App, c: char) {
    app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
}
fn tree_prompt(app: &mut App, letter: char) {
    if app.focus() != Focus::Tree {
        ctrl(app, 'w');
    }
    key(app, KeyCode::Char(letter));
}
fn search_notes(app: &mut App) {
    ctrl(app, 'p');
    key(app, KeyCode::Tab);
}
fn shift(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::SHIFT));
}
fn draw(app: &mut App) -> Vec<String> {
    draw_size(app, 100, 30)
}
fn draw_size(app: &mut App, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..buffer.area.height)
        .map(|y| {
            // Wide characters are followed by a filler cell; skip it.
            let mut line = String::new();
            let mut x = 0;
            while x < buffer.area.width {
                let symbol = buffer[(x, y)].symbol();
                line.push_str(symbol);
                x += UnicodeWidthStr::width(symbol).max(1) as u16;
            }
            line
        })
        .collect()
}

fn click_header(app: &mut App, screen: &[String], label: &str) {
    let prefix = screen[2].split_once(label).unwrap().0;
    mouse_at(app, UnicodeWidthStr::width(prefix) as u16, 2);
}

fn mouse_at(app: &mut App, column: u16, row: u16) {
    for kind in [
        MouseEventKind::Down(crossterm::event::MouseButton::Left),
        MouseEventKind::Up(crossterm::event::MouseButton::Left),
    ] {
        app.mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }
}

#[test]
fn create_in_tree_defaults_to_markdown_and_creates_folders() {
    let f = fixture(&[("a.md", "original")]);
    let mut app = f.app(Some("a.md"));
    key(&mut app, KeyCode::Char('a'));
    key(&mut app, KeyCode::Char('A'));
    assert!(!app.has_prompt(), "letters still edit the document");
    assert_eq!(app.note_text(), Some("aAoriginal"));
    ctrl(&mut app, 'w');
    key(&mut app, KeyCode::Char('a'));
    app.paste("新笔记");
    let screen = draw(&mut app).join("\n");
    assert!(screen.contains(".md 笔记") && screen.contains("/ 文件夹"));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.note_path(), Some(f.path("新笔记.md").as_path()));
    assert_eq!(fs::read_to_string(f.path("a.md")).unwrap(), "aAoriginal");

    ctrl(&mut app, 'w');
    shift(&mut app, KeyCode::Char('A'));
    app.paste("资料/");
    key(&mut app, KeyCode::Enter);
    assert!(f.path("资料").is_dir());
    assert_eq!(app.focus(), Focus::Tree);
    assert_eq!(app.note_path(), Some(f.path("新笔记.md").as_path()));

    for name in ["第一篇.md", "第二篇.MD", "笔记.MARKDOWN"] {
        // Creating inside a selected folder, then alongside its selected file.
        if app.focus() == Focus::Editor {
            ctrl(&mut app, 'w');
        }
        key(&mut app, KeyCode::Char('A'));
        app.paste(name);
        key(&mut app, KeyCode::Enter);
        assert!(!app.has_prompt(), "{:?}", app.message());
        assert!(f.path(&format!("资料/{name}")).is_file());
        assert!(app.tree_paths().contains(&f.path(&format!("资料/{name}"))));
    }
    assert_eq!(
        app.note_path(),
        Some(f.path("资料/笔记.MARKDOWN").as_path())
    );
    tree_prompt(&mut app, 'a');
    app.paste("另一篇");
    key(&mut app, KeyCode::Enter);
    assert!(f.path("资料/另一篇.md").is_file());
    tree_prompt(&mut app, 'a');
    app.paste("子目录/");
    key(&mut app, KeyCode::Enter);
    assert!(f.path("资料/子目录").is_dir());
}

#[test]
fn create_rejects_invalid_names_and_collisions_without_overwriting() {
    let f = fixture(&[("a.md", "keep"), ("data.xxx", "preserve")]);
    fs::create_dir(f.path("folder")).unwrap();
    let mut app = f.app(Some("a.md"));
    ctrl(&mut app, 'w');
    for name in [
        "",
        "/",
        "../escape",
        "../",
        "nested/name",
        "folder//",
        ".hidden",
        "a",
        "data.xxx",
        "folder/",
    ] {
        key(&mut app, KeyCode::Char('a'));
        app.paste(name);
        key(&mut app, KeyCode::Enter);
        assert!(app.has_prompt(), "should reject {name:?}");
        assert!(app.message().is_some());
        key(&mut app, KeyCode::Esc);
    }
    assert_eq!(fs::read_to_string(f.path("a.md")).unwrap(), "keep");
    assert_eq!(fs::read_to_string(f.path("data.xxx")).unwrap(), "preserve");
    assert_eq!(fs::read_dir(&f.root).unwrap().count(), 3);
    key(&mut app, KeyCode::Char('a'));
    app.paste("cancelled");
    key(&mut app, KeyCode::Esc);
    assert!(!f.path("cancelled.md").exists());
}

#[test]
fn sidebar_click_keeps_focus_for_create_and_enter_moves_to_editor() {
    let f = fixture(&[("a.md", "A"), ("b.md", "B")]);
    let mut app = f.app(Some("a.md"));
    mouse_at(&mut app, 5, 4); // b.md, after opening a.md at startup
    assert_eq!(app.note_path(), Some(f.path("b.md").as_path()));
    assert_eq!(app.focus(), Focus::Tree);
    key(&mut app, KeyCode::Char('a'));
    assert!(app.has_prompt());
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.focus(), Focus::Editor);
    for (column, row) in [(5, 15), (3, 2), (0, 8)] {
        draw(&mut app);
        mouse_at(&mut app, column, row);
        assert_eq!(app.focus(), Focus::Tree);
        key(&mut app, KeyCode::Char('A'));
        assert!(app.has_prompt());
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.focus(), Focus::Editor);
    }
    mouse_at(&mut app, 5, 3);
    mouse_at(&mut app, 5, 3);
    assert_eq!(app.focus(), Focus::Editor, "double click starts editing");
    assert_eq!(app.note_path(), Some(f.path("a.md").as_path()));

    ctrl(&mut app, 'w');
    draw_size(&mut app, 50, 20);
    mouse_at(&mut app, 5, 4);
    assert_eq!(app.focus(), Focus::Tree, "narrow view stays in the list");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.focus(), Focus::Editor);
    assert_eq!(app.note_path(), Some(f.path("b.md").as_path()));
}

#[test]
fn empty_sidebar_and_modal_clicks_have_correct_focus() {
    let f = fixture(&[]);
    let mut app = f.app(None);
    key(&mut app, KeyCode::Tab);
    draw(&mut app);
    mouse_at(&mut app, 5, 12);
    assert_eq!(app.focus(), Focus::Tree);
    key(&mut app, KeyCode::Char('a'));
    app.paste("first");
    key(&mut app, KeyCode::Enter);
    assert!(f.path("first.md").exists());
    draw(&mut app);
    ctrl(&mut app, 'p');
    mouse_at(&mut app, 5, 12);
    assert_eq!(app.focus(), Focus::Editor, "prompt blocks background focus");
    key(&mut app, KeyCode::Esc);
    draw_size(&mut app, 20, 8);
    mouse_at(&mut app, 5, 12);
    assert_eq!(
        app.focus(),
        Focus::Editor,
        "small frames clear mouse targets"
    );
}

#[test]
fn non_markdown_extensions_are_rejected_and_prompt_can_be_corrected() {
    let f = fixture(&[("a.md", "A")]);
    let mut app = f.app(Some("a.md"));
    for name in ["data.xxx", "config.json", "notes.txt", "trailing."] {
        tree_prompt(&mut app, 'a');
        app.paste(name);
        key(&mut app, KeyCode::Enter);
        assert!(app.has_prompt());
        assert!(app.message().unwrap().contains("只支持新建 Markdown"));
        assert!(!f.path(name).exists());
        assert!(!f.path(&format!("{name}.md")).exists());
        key(&mut app, KeyCode::Esc);
    }
    tree_prompt(&mut app, 'a');
    app.paste("draft.txt");
    key(&mut app, KeyCode::Enter);
    ctrl(&mut app, 'a');
    app.paste("draft.md");
    key(&mut app, KeyCode::Enter);
    assert!(!app.has_prompt());
    assert_eq!(app.note_path(), Some(f.path("draft.md").as_path()));
}

#[test]
fn tree_creation_binding_can_be_changed_without_intercepting_editor_text() {
    use linknote::app::keymap::{Action, Keymap, Scope};
    let f = fixture(&[("a.md", "A")]);
    let path = f.base.join("config/keys.json");
    let mut map = Keymap::load(&path);
    let binding = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);
    map.assign(&path, Action::Create, Some(binding)).unwrap();
    assert_eq!(map.action(&binding, Scope::Tree), Some(Action::Create));
    assert_eq!(map.action(&binding, Scope::Editor), None);
    assert!(map.assign(&path, Action::Save, Some(binding)).is_err());
    let mut app = f.app(Some("a.md"));
    key(&mut app, KeyCode::Char('n'));
    assert_eq!(app.note_text(), Some("nA"));
    ctrl(&mut app, 'w');
    key(&mut app, KeyCode::Char('a'));
    assert!(!app.has_prompt());
    key(&mut app, KeyCode::Char('n'));
    assert!(app.has_prompt());
    map.assign(&path, Action::Create, None).unwrap();
    assert_eq!(
        map.action(
            &KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
            Scope::Tree
        ),
        Some(Action::Create)
    );
}

#[test]
fn sidebar_button_and_shortcut_preserve_note_selection_and_width() {
    let f = fixture(&[("folder/a.md", "正文与光标"), ("b.md", "另一篇")]);
    let mut app = f.app(Some("folder/a.md"));
    ctrl(&mut app, ',');
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Esc);
    shift(&mut app, KeyCode::Right);
    let cursor = app.cursor();
    let selection = app.selection();
    let screen = draw(&mut app);
    let tree_row = screen[3].clone();
    click_header(&mut app, &screen, "<");
    assert_eq!(app.focus(), Focus::Editor);
    let hidden = draw(&mut app);
    assert!(hidden[2].contains(">"));
    assert!(!hidden[2].contains("收起"));
    assert_eq!(app.cursor(), cursor);
    assert_eq!(app.selection(), selection);
    click_header(&mut app, &hidden, ">");
    assert_eq!(draw(&mut app)[3], tree_row);
    assert_eq!(app.selection(), selection);
    ctrl(&mut app, 'w');
    ctrl(&mut app, 'b');
    assert_eq!(app.focus(), Focus::Editor);
    assert!(draw(&mut app)[2].contains(">"));
    ctrl(&mut app, 'b');
    assert_eq!(draw(&mut app)[3], tree_row);
    assert_eq!(app.note_text(), Some("正文与光标"));
    assert_eq!(app.note_path(), Some(f.path("folder/a.md").as_path()));
}

#[test]
fn sidebar_can_be_opened_in_a_narrow_window_and_ignores_modal_clicks() {
    let f = fixture(&[("a.md", "正文")]);
    let mut app = f.app(Some("a.md"));
    let narrow = draw_size(&mut app, 44, 16);
    click_header(&mut app, &narrow, ">");
    assert_eq!(app.focus(), Focus::Tree);
    let narrow = draw_size(&mut app, 44, 16);
    assert!(narrow[2].contains("<"));
    click_header(&mut app, &narrow, "<");
    assert_eq!(app.focus(), Focus::Editor);
    draw_size(&mut app, 44, 16);
    ctrl(&mut app, 'b');
    assert_eq!(app.focus(), Focus::Tree);
    draw_size(&mut app, 44, 16);
    ctrl(&mut app, 'b');
    assert_eq!(app.focus(), Focus::Editor);
    let hidden = draw(&mut app);
    key(&mut app, KeyCode::F(1));
    click_header(&mut app, &hidden, ">");
    key(&mut app, KeyCode::Esc);
    assert!(draw(&mut app)[2].contains(">"));
    // Too-small frames clear obsolete mouse targets.
    draw_size(&mut app, 20, 8);
    click_header(&mut app, &hidden, ">");
    assert!(draw(&mut app)[2].contains(">"));
    assert_eq!(app.note_text(), Some("正文"));
}

#[test]
fn sidebar_button_works_without_notes_or_a_keyboard_binding() {
    let f = fixture(&[]);
    fs::create_dir_all(f.base.join("config")).unwrap();
    fs::write(
        f.base.join("config/keys.json"),
        r#"{"keys":{"toggle_sidebar":[]}}"#,
    )
    .unwrap();
    let mut app = f.app(None);
    let screen = draw(&mut app);
    click_header(&mut app, &screen, "<");
    let hidden = draw(&mut app);
    ctrl(&mut app, 'b');
    assert_eq!(draw(&mut app)[2], hidden[2]);
    click_header(&mut app, &hidden, ">");
    assert!(draw(&mut app)[2].contains("<"));
}
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}
fn wait_for(app: &mut App, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(app) && Instant::now() < deadline {
        app.tick();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(done(app), "timed out");
}

#[test]
fn bottom_shortcut_bar_is_gone_and_messages_use_the_status_line() {
    let f = fixture(&[("a.md", "# 标题\n\n正文\n")]);
    let mut app = f.app(None);
    let screen = draw(&mut app);
    let text = screen.join("\n");
    assert!(!text.contains("Ctrl+P 打开"), "{text}");
    assert!(screen.last().unwrap().contains("已保存"), "{text}");
    ctrl(&mut app, 's');
    let screen = draw(&mut app);
    assert!(
        screen
            .last()
            .unwrap()
            .contains("已保存（LinkNote 会自动保存")
    );
    // The editor gets the freed row: its border now ends on the second-to-last line.
    assert!(screen[screen.len() - 2].contains('└'));
}

#[test]
fn r_renames_tree_files_and_folders_but_types_in_editor_and_prompt() {
    for target in ["a.md", "folder"] {
        let f = fixture(&[("a.md", "A"), ("folder/child.md", "child")]);
        let mut app = f.app(Some("a.md"));
        key(&mut app, KeyCode::Char('r'));
        assert_eq!(app.note_text(), Some("rA"));
        assert!(!app.has_prompt());
        ctrl(&mut app, 'w');
        if target == "folder" {
            key(&mut app, KeyCode::Home);
        }
        key(&mut app, KeyCode::Char('r'));
        assert!(app.has_prompt());
        ctrl(&mut app, 'a');
        key(&mut app, KeyCode::Char('r'));
        app.paste("enamed");
        key(&mut app, KeyCode::Enter);
        assert!(!app.has_prompt());
        assert!(!f.path(target).exists());
        let (path, contents) = if target == "folder" {
            ("renamed/child.md", "child")
        } else {
            ("renamed.md", "rA")
        };
        assert_eq!(fs::read_to_string(f.path(path)).unwrap(), contents);
    }
}

#[test]
fn c_copies_selected_absolute_path_without_editing_or_changing_focus() {
    let f = fixture(&[("a.md", "A"), ("资料 空格/中文.md", "child")]);
    let mut app = f.app(Some("a.md"));
    key(&mut app, KeyCode::Char('c'));
    assert_eq!(app.note_text(), Some("cA"));
    ctrl(&mut app, 'w');
    for target in ["a.md", "资料 空格", "资料 空格/中文.md"] {
        if target == "资料 空格" {
            key(&mut app, KeyCode::Home);
        } else if target.ends_with("中文.md") {
            key(&mut app, KeyCode::Right);
            key(&mut app, KeyCode::Down);
        }
        key(&mut app, KeyCode::Char('c'));
        assert_eq!(app.focus(), Focus::Tree);
        assert_eq!(app.note_text(), Some("cA"));
        assert_eq!(app.message(), Some("已复制绝对路径"));
        // Inspect through the normal paste path without touching the system clipboard.
        tree_prompt(&mut app, 'a');
        ctrl(&mut app, 'v');
        key(&mut app, KeyCode::Char('c')); // c must still type in a prompt.
        ctrl(&mut app, 'a');
        ctrl(&mut app, 'c');
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Tab);
        ctrl(&mut app, 'a');
        ctrl(&mut app, 'v');
        assert_eq!(
            app.note_text(),
            Some(format!("{}c", f.path(target).display()).as_str())
        );
        ctrl(&mut app, 'z');
        assert_eq!(app.note_text(), Some("cA"));
        ctrl(&mut app, 'w');
    }
}

#[test]
fn case_only_rename_works_on_case_insensitive_file_systems() {
    let f = fixture(&[("note.md", "内容")]);
    let mut app = f.app(Some("note.md"));
    tree_prompt(&mut app, 'r');
    ctrl(&mut app, 'u');
    app.paste("Note.md");
    key(&mut app, KeyCode::Enter);
    assert!(!app.has_prompt(), "{:?}", app.message());
    assert_eq!(names(&f.root), ["Note.md"]);
    assert_eq!(app.note_path(), Some(f.path("Note.md").as_path()));
    key(&mut app, KeyCode::Tab);
    app.paste("改");
    ctrl(&mut app, 's');
    assert_eq!(fs::read_to_string(f.path("Note.md")).unwrap(), "改内容");
}

#[test]
fn a_conflicted_note_does_not_trap_the_user_and_can_be_resolved_later() {
    let f = fixture(&[("a.md", "原文"), ("b.md", "另一篇")]);
    let mut app = f.app(Some("a.md"));
    app.paste("本地");
    fs::write(f.path("a.md"), "外部修改").unwrap();
    // Opening another note used to fail until the conflict was resolved.
    ctrl(&mut app, 'p');
    app.paste("b.md");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.note_path(), Some(f.path("b.md").as_path()));
    assert!(app.message().unwrap().contains("恢复草稿"));
    assert_eq!(fs::read_to_string(f.path("a.md")).unwrap(), "外部修改");
    assert!(
        draw(&mut app).join("\n").contains("a.md ●"),
        "draft marker in tree"
    );

    ctrl(&mut app, 'p');
    app.paste("a.md");
    key(&mut app, KeyCode::Enter);
    assert!(app.has_dialog(), "recovery prompt");
    key(&mut app, KeyCode::Enter);
    assert!(app.conflict());
    assert_eq!(app.note_text(), Some("本地原文"));
    ctrl(&mut app, 'r');
    app.key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    let screen = draw(&mut app).join("\n");
    assert!(
        screen.contains("- 外部修改") && screen.contains("+ 本地原文"),
        "{screen}"
    );
    key(&mut app, KeyCode::Esc);
    app.key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
    assert!(!app.conflict());
    assert_eq!(app.note_text(), Some("外部修改"));
    ctrl(&mut app, 'z');
    assert_eq!(
        app.note_text(),
        Some("本地原文"),
        "local text is one undo away"
    );
}

#[test]
fn find_and_replace_with_smart_case() {
    let f = fixture(&[("a.md", "Apple apple APPLE\n")]);
    let mut app = f.app(None);
    ctrl(&mut app, 'f');
    app.paste("apple");
    assert!(draw(&mut app).join("\n").contains("3 处匹配"));
    key(&mut app, KeyCode::Tab);
    app.paste("梨");
    key(&mut app, KeyCode::Enter); // select the first match
    key(&mut app, KeyCode::Enter); // replace it and select the next
    assert_eq!(app.note_text(), Some("梨 apple APPLE\n"));
    ctrl(&mut app, 'r');
    assert_eq!(app.note_text(), Some("梨 梨 梨\n"));
    assert!(app.message().unwrap().contains("已替换 2 处"));
    ctrl(&mut app, 'z');
    assert_eq!(app.note_text(), Some("梨 apple APPLE\n"));
}

#[test]
fn full_text_search_opens_the_match() {
    let f = fixture(&[
        ("a.md", "开头\n"),
        ("目录/b.md", "第一行\n这里提到独特词汇。\n"),
    ]);
    let mut app = f.app(Some("a.md"));
    search_notes(&mut app);
    app.paste("独特词汇");
    wait_for(&mut app, |a| !a.prompt_choices().is_empty());
    assert_eq!(app.prompt_choices(), ["目录/b.md:2 这里提到独特词汇。"]);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.note_path(), Some(f.path("目录/b.md").as_path()));
    let text = app.note_text().unwrap().to_owned();
    assert_eq!(&text[app.selection().unwrap()], "独特词汇");
    // Tab switches the same query to file names.
    search_notes(&mut app);
    app.paste("b.md");
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.prompt_choices(), ["目录/b.md"]);
}

#[test]
fn links_open_notes_headings_and_create_missing_notes() {
    let f = fixture(&[
        (
            "a.md",
            "见 [第二节](sub/b.md#第二节)，[[深处]] 和 [[新想法]]\n",
        ),
        ("sub/b.md", "# 第一节\n\n内容\n\n## 第二节\n\n更多\n"),
        ("sub/更深/深处.md", "深处的笔记\n"),
    ]);
    let mut app = f.app(Some("a.md"));
    let text = app.note_text().unwrap().to_owned();
    let click_offset = |needle: &str| text.find(needle).unwrap() + 1;
    // Place the cursor in a link and follow it with F12.
    app.paste("");
    move_cursor(&mut app, click_offset("第二节"));
    key(&mut app, KeyCode::F(12));
    assert_eq!(app.note_path(), Some(f.path("sub/b.md").as_path()));
    let b = app.note_text().unwrap().to_owned();
    assert_eq!(app.cursor(), b.find("## 第二节"));
    shift(&mut app, KeyCode::F(12));
    assert_eq!(app.note_path(), Some(f.path("a.md").as_path()));

    move_cursor(&mut app, click_offset("深处"));
    key(&mut app, KeyCode::F(12));
    assert_eq!(app.note_path(), Some(f.path("sub/更深/深处.md").as_path()));
    shift(&mut app, KeyCode::F(12));

    move_cursor(&mut app, click_offset("新想法"));
    key(&mut app, KeyCode::F(12));
    assert!(app.has_prompt());
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.note_path(), Some(f.path("新想法.md").as_path()));
}

fn move_cursor(app: &mut App, offset: usize) {
    key(app, KeyCode::Home);
    app.key(KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL));
    while app.cursor().unwrap() < offset {
        key(app, KeyCode::Right);
    }
}

#[test]
fn delete_moves_to_trash_and_move_changes_folder() {
    let f = fixture(&[("a.md", "A"), ("b.md", "B"), ("归档/.keep.md", "")]);
    let mut app = f.app(Some("a.md"));
    ctrl(&mut app, 'w'); // focus the tree, selection on a.md
    tree_prompt(&mut app, 'm');
    app.paste("归档");
    assert_eq!(app.prompt_choices(), ["归档"]);
    key(&mut app, KeyCode::Enter);
    assert!(f.path("归档/a.md").exists() && !f.path("a.md").exists());
    assert_eq!(app.note_path(), Some(f.path("归档/a.md").as_path()));
    ctrl(&mut app, 'w');
    app.paste("移动后");
    ctrl(&mut app, 's');
    assert_eq!(fs::read_to_string(f.path("归档/a.md")).unwrap(), "移动后A");

    ctrl(&mut app, 'w');
    let paths = app.tree_paths();
    let b = paths.iter().position(|p| p.ends_with("b.md")).unwrap();
    key(&mut app, KeyCode::Home);
    for _ in 0..b {
        key(&mut app, KeyCode::Down);
    }
    key(&mut app, KeyCode::Char('d'));
    assert!(app.has_dialog());
    key(&mut app, KeyCode::Enter);
    assert!(!f.path("b.md").exists());
    assert_eq!(fs::read_to_string(f.base.join("trash/b.md")).unwrap(), "B");
}

#[test]
fn d_and_shift_d_only_trash_after_confirmation_and_do_not_intercept_typing() {
    for binding in [
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('D'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT),
    ] {
        for target in ["a.md", "folder"] {
            let f = fixture(&[("a.md", "A"), ("folder/child.md", "child")]);
            let mut app = f.app(Some("a.md"));
            key(&mut app, KeyCode::Char('d'));
            shift(&mut app, KeyCode::Char('D'));
            assert_eq!(app.note_text(), Some("dDA"));
            assert!(!app.has_dialog());
            ctrl(&mut app, 'w');
            if target == "folder" {
                key(&mut app, KeyCode::Home);
            }
            app.key(binding);
            assert!(app.has_dialog());
            let screen = draw(&mut app).join("\n");
            assert!(screen.contains("移到废纸篓") && screen.contains(target));
            app.key(binding); // Repeating d/D must never confirm the deletion.
            assert!(app.has_dialog());
            assert!(f.path(target).exists());
            key(&mut app, KeyCode::Esc);
            assert!(!app.has_dialog());
            assert!(f.path(target).exists());
            assert!(!f.base.join("trash").exists());
            app.key(binding);
            key(&mut app, KeyCode::Enter);
            assert!(!f.path(target).exists());
            let (trashed, contents) = if target == "folder" {
                ("trash/folder/child.md", "child")
            } else {
                ("trash/a.md", "dDA")
            };
            assert_eq!(fs::read_to_string(f.base.join(trashed)).unwrap(), contents);
        }
    }
}

#[test]
fn minimal_default_keys_drop_legacy_aliases_but_allow_explicit_rebinding() {
    use linknote::app::keymap::{Action, Keymap, Scope};
    let mut map = Keymap::default();
    for code in (2..=9).map(KeyCode::F).chain([KeyCode::Delete]) {
        assert_eq!(
            map.action(&KeyEvent::new(code, KeyModifiers::NONE), Scope::Tree),
            None
        );
    }
    for c in ['n', 'o'] {
        let event = KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert_eq!(map.action(&event, Scope::Tree), None);
        assert_eq!(map.action(&event, Scope::Editor), None);
    }
    for (c, action) in [
        ('a', Action::Create),
        ('r', Action::RenameTree),
        ('c', Action::CopyPath),
        ('d', Action::Delete),
        ('m', Action::MoveTree),
    ] {
        let event = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        assert_eq!(map.action(&event, Scope::Tree), Some(action));
        assert_eq!(map.action(&event, Scope::Editor), None);
    }
    let f = fixture(&[("a.md", "A")]);
    let path = f.base.join("config/keys.json");
    let ctrl_n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
    map.assign(&path, Action::NewNote, Some(ctrl_n)).unwrap();
    let mut app = f.app(None);
    app.key(ctrl_n);
    assert!(
        app.has_prompt(),
        "an explicitly configured old binding still works"
    );
    key(&mut app, KeyCode::Esc);
    map.assign(&path, Action::NewNote, None).unwrap();
    assert_eq!(map.action(&ctrl_n, Scope::Editor), None);
}

#[test]
fn key_bindings_can_be_configured() {
    let f = fixture(&[("a.md", "A")]);
    fs::create_dir_all(f.base.join("config")).unwrap();
    fs::write(
        f.base.join("config/keys.json"),
        r#"{"keys": {"search_notes": ["f4"], "nonsense": ["f9"]}}"#,
    )
    .unwrap();
    let mut app = f.app(None);
    assert!(app.message().unwrap().contains("未知动作 nonsense"));
    ctrl(&mut app, 'o');
    assert!(!app.has_prompt(), "default binding replaced");
    key(&mut app, KeyCode::F(4));
    assert!(app.has_prompt());
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::F(1));
    let help = draw(&mut app).join("\n");
    assert!(help.contains("F4") && help.contains("全文搜索"), "{help}");
}

#[test]
fn esc_clears_the_selection_before_leaving_the_editor() {
    let f = fixture(&[("a.md", "文字")]);
    let mut app = f.app(None);
    ctrl(&mut app, 'a');
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.focus(), Focus::Editor);
    assert!(app.selection().is_none());
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.focus(), Focus::Tree);
}

#[test]
fn mouse_wheel_scrolls_the_tree_without_moving_the_selection() {
    let files: Vec<(String, &str)> = (0..60).map(|i| (format!("{i:02}.md"), "")).collect();
    let files: Vec<(&str, &str)> = files.iter().map(|(n, t)| (n.as_str(), *t)).collect();
    let f = fixture(&files);
    let mut app = f.app(Some("00.md"));
    let in_tree = |screen: &[String], name: &str| {
        screen.iter().any(|l| l.starts_with(&format!("│   {name}")))
    };
    assert!(in_tree(&draw(&mut app), "00.md"));
    for _ in 0..3 {
        app.mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 5,
            row: 6,
            modifiers: KeyModifiers::NONE,
        });
    }
    assert!(!in_tree(&draw(&mut app), "00.md"), "tree scrolled");
    assert_eq!(app.note_path(), Some(f.path("00.md").as_path()));
    // Keyboard navigation brings the selection back into view.
    ctrl(&mut app, 'w');
    key(&mut app, KeyCode::Up);
    assert!(in_tree(&draw(&mut app), "00.md"));
}

#[test]
fn ctrl_s_saves_immediately() {
    let f = fixture(&[("a.md", "A")]);
    let mut app = f.app(None);
    app.paste("新");
    ctrl(&mut app, 's');
    assert_eq!(fs::read_to_string(f.path("a.md")).unwrap(), "新A");
}

#[test]
fn settings_entry_changes_app_theme_persists_and_preserves_note_and_cursor() {
    let f = fixture(&[("a.md", "# 标题\n\n**粗体** [链接](b.md)\n")]);
    let mut app = f.app(Some("a.md"));
    let text = app.note_text().unwrap().to_string();
    let cursor = app.cursor();
    assert!(draw(&mut app)[0].contains("设置 Ctrl+,"));
    app.mouse(MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: 94,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.has_dialog());
    assert!(draw(&mut app).join("\n").contains("整体主题"));
    key(&mut app, KeyCode::Right);
    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(f.base.join("config/settings.json")).unwrap()).unwrap();
    assert_eq!(settings["theme"], "light");
    app.paste("不得写入笔记");
    key(&mut app, KeyCode::Esc);
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    assert_eq!(
        terminal.backend().buffer()[(0, 0)].bg,
        ratatui::style::Color::Rgb(247, 248, 250)
    );
    let bold = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .find(|c| c.symbol() == "粗")
        .unwrap();
    assert_eq!(bold.fg, ratatui::style::Color::Rgb(15, 25, 38));
    assert_eq!(app.note_text(), Some(text.as_str()));
    assert_eq!(app.cursor(), cursor);
    assert_eq!(fs::read_to_string(f.path("a.md")).unwrap(), text);
    drop(app);
    let mut app = f.app(Some("a.md"));
    ctrl(&mut app, ',');
    assert!(draw(&mut app).join("\n").contains("浅色"));
}

#[test]
fn settings_shortcuts_capture_conflicts_restore_and_survive_restart() {
    let f = fixture(&[("a.md", "笔记")]);
    let mut app = f.app(Some("a.md"));
    ctrl(&mut app, ',');
    key(&mut app, KeyCode::Tab);
    assert!(draw(&mut app).join("\n").contains("Mermaid 主题"));
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Enter); // capture Help
    ctrl(&mut app, ','); // conflicts with settings
    assert!(draw(&mut app).join("\n").contains("已用于"));
    assert!(!f.base.join("config/keys.json").exists());
    key(&mut app, KeyCode::F(10)); // now reserved for the image viewer
    assert!(draw(&mut app).join("\n").contains("放大图片"));
    key(&mut app, KeyCode::F(11));
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::F(1));
    assert!(!app.has_dialog());
    key(&mut app, KeyCode::F(11));
    assert!(app.has_dialog());
    key(&mut app, KeyCode::Esc);
    drop(app);
    let mut app = f.app(Some("a.md"));
    key(&mut app, KeyCode::F(11));
    assert!(app.has_dialog());
    key(&mut app, KeyCode::Esc);
    ctrl(&mut app, ',');
    key(&mut app, KeyCode::BackTab);
    key(&mut app, KeyCode::Backspace);
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::F(1));
    assert!(app.has_dialog());
    assert_eq!(app.note_text(), Some("笔记"));
}

#[test]
fn settings_save_failure_keeps_current_theme_and_stays_usable_at_small_sizes() {
    let f = fixture(&[("a.md", "原文")]);
    let mut app = f.app(Some("a.md"));
    fs::write(f.base.join("config"), "not a directory").unwrap();
    ctrl(&mut app, ',');
    key(&mut app, KeyCode::Right);
    assert!(draw(&mut app).join("\n").contains("未覆盖"));
    for (w, h) in [(34, 9), (48, 14), (100, 30)] {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert_eq!(
            terminal.backend().buffer()[(0, 0)].bg,
            ratatui::style::Color::Rgb(22, 27, 34)
        );
    }
    key(&mut app, KeyCode::Esc);
    assert!(!app.has_dialog());
    assert_eq!(app.note_text(), Some("原文"));
}

#[test]
fn mermaid_canvas_opens_source_and_edits_autosave_without_rewriting_markdown() {
    let source = "# Diagram\n\n```mermaid\nflowchart LR\n A[Start] --> B[End]\n```\n\nTail\n";
    let f = fixture(&[("a.md", source)]);
    let mut app = f.app(Some("a.md"));
    let screen = draw(&mut app);
    let y = screen
        .iter()
        .position(|line| line.contains("Mermaid ·"))
        .unwrap();
    // Canvas rows and their caption both map to the original fence.
    app.mouse(MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: 40,
        row: (y + 1) as u16,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.cursor(), Some(source.find("```mermaid").unwrap()));
    assert!(draw(&mut app).join("\n").contains("```mermaid"));
    assert_eq!(app.note_text(), Some(source));
    ctrl(&mut app, 'f');
    app.paste("Start");
    key(&mut app, KeyCode::Enter);
    app.paste("中文起点");
    wait_for(&mut app, |_| {
        fs::read_to_string(f.path("a.md"))
            .unwrap()
            .contains("中文起点")
    });
    assert_eq!(
        fs::read_to_string(f.path("a.md")).unwrap(),
        source.replace("Start", "中文起点")
    );
    ctrl(&mut app, 'z');
    assert_eq!(app.note_text(), Some(source));
    app.key(KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL));
    let screen = draw(&mut app).join("\n");
    assert!(screen.contains("Mermaid ·"));
    assert!(!screen.contains("```mermaid"));
}

#[test]
fn a_corrupt_recovery_draft_is_set_aside_and_the_note_opens() {
    let f = fixture(&[("a.md", "原文")]);
    let recovery = f.base.join("state/recovery");
    fs::create_dir_all(&recovery).unwrap();
    let draft = linknote::storage::recovery_path(&recovery, &f.path("a.md"));
    fs::write(&draft, "{损坏").unwrap();
    let app = f.app(Some("a.md"));
    assert_eq!(app.note_text(), Some("原文"));
    assert!(!app.has_dialog());
    assert!(app.message().unwrap().contains("恢复草稿已损坏"));
    assert!(!draft.exists());
    let aside = draft.with_extension("corrupt-1.json");
    assert_eq!(
        fs::read_to_string(aside).unwrap(),
        "{损坏",
        "kept for the user"
    );
}

#[cfg(unix)]
#[test]
fn a_failing_recovery_draft_does_not_stop_saving() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture(&[("a.md", "原文")]);
    let mut app = f.app(Some("a.md"));
    let recovery = f.base.join("state/recovery");
    fs::set_permissions(&recovery, fs::Permissions::from_mode(0o555)).unwrap();
    app.paste("本地");
    let mut warned = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while fs::read_to_string(f.path("a.md")).unwrap() != "本地原文" && Instant::now() < deadline
    {
        app.tick();
        warned |= draw(&mut app).join("\n").contains("恢复草稿写入失败");
        std::thread::sleep(Duration::from_millis(20));
    }
    let saved = fs::read_to_string(f.path("a.md")).unwrap();
    fs::set_permissions(&recovery, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(warned, "the draft failure is shown");
    assert_eq!(saved, "本地原文");
}

fn modified(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    app.key(KeyEvent::new(code, modifiers));
}

fn editing_modifiers() -> Vec<KeyModifiers> {
    let mut modifiers = vec![KeyModifiers::CONTROL];
    if cfg!(target_os = "macos") {
        modifiers.push(KeyModifiers::SUPER);
    }
    modifiers
}

#[test]
fn command_and_control_edit_the_document_and_share_the_clipboard_with_prompts() {
    for modifier in editing_modifiers() {
        let f = fixture(&[("a.md", "中文 👨‍👩‍👧‍👦 e\u{301}\nsecond")]);
        let mut app = f.app(None);
        let original = app.note_text().unwrap().to_owned();
        modified(&mut app, KeyCode::Char('a'), modifier);
        modified(&mut app, KeyCode::Char('c'), modifier);
        assert_eq!(app.note_text(), Some(original.as_str()));
        modified(&mut app, KeyCode::Char('x'), modifier);
        assert_eq!(app.note_text(), Some(""));
        modified(&mut app, KeyCode::Char('z'), modifier);
        assert_eq!(app.note_text(), Some(original.as_str()));
        modified(&mut app, KeyCode::Char('z'), modifier | KeyModifiers::SHIFT);
        assert_eq!(app.note_text(), Some(""));
        modified(&mut app, KeyCode::Char('v'), modifier);
        assert_eq!(app.note_text(), Some(original.as_str()));
        tree_prompt(&mut app, 'r');
        modified(&mut app, KeyCode::Char('a'), modifier);
        // Select-all must not destroy the original name before another edit.
        assert!(draw(&mut app).join("\n").contains("a.md"));
        modified(&mut app, KeyCode::Char('v'), modifier);
        assert!(
            draw(&mut app)
                .join("\n")
                .contains("中文 👨‍👩‍👧‍👦 e\u{301} second")
        );
        modified(&mut app, KeyCode::Char('z'), modifier);
        assert!(draw(&mut app).join("\n").contains("> a.md"));
        modified(&mut app, KeyCode::Char('a'), modifier);
        app.paste("新名字");
        modified(&mut app, KeyCode::Char('a'), modifier);
        modified(&mut app, KeyCode::Char('c'), modifier);
        modified(&mut app, KeyCode::Char('x'), modifier);
        modified(&mut app, KeyCode::Char('z'), modifier);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.note_path(), Some(f.path("新名字.md").as_path()));
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.note_text(), Some(original.as_str()));
        modified(&mut app, KeyCode::Char('a'), modifier);
        modified(&mut app, KeyCode::Char('v'), modifier);
        assert_eq!(app.note_text(), Some("新名字"));
        modified(&mut app, KeyCode::Char('z'), modifier);
        assert_eq!(app.note_text(), Some(original.as_str()));
    }
}

#[test]
fn search_and_replacement_have_independent_selection_and_undo_histories() {
    for modifier in editing_modifiers() {
        let f = fixture(&[("a.md", "apple pear apple")]);
        let mut app = f.app(None);
        ctrl(&mut app, 'f');
        app.paste("apple");
        key(&mut app, KeyCode::Enter);
        ctrl(&mut app, 'f'); // Existing query is selected, but copying is not an edit.
        modified(&mut app, KeyCode::Char('c'), modifier);
        assert!(draw(&mut app).join("\n").contains("2 处匹配"));
        key(&mut app, KeyCode::Tab);
        modified(&mut app, KeyCode::Char('v'), modifier);
        modified(&mut app, KeyCode::Char('a'), modifier);
        app.paste("梨");
        modified(&mut app, KeyCode::Char('z'), modifier);
        assert!(draw(&mut app).join("\n").contains("替换 > apple"));
        modified(&mut app, KeyCode::Char('z'), modifier | KeyModifiers::SHIFT);
        assert!(draw(&mut app).join("\n").contains("替换 > 梨"));
        key(&mut app, KeyCode::Tab);
        app.paste("pear");
        assert!(draw(&mut app).join("\n").contains("1 处匹配"));
        modified(&mut app, KeyCode::Char('z'), modifier);
        assert!(draw(&mut app).join("\n").contains("2 处匹配"));
        assert_eq!(app.note_text(), Some("apple pear apple"));
        ctrl(&mut app, 'r');
        assert_eq!(app.note_text(), Some("梨 pear 梨"));
        modified(&mut app, KeyCode::Char('z'), modifier);
        assert_eq!(app.note_text(), Some("apple pear apple"));
    }
}

#[test]
fn prompt_editing_preserves_graphemes_and_keeps_cursor_visible() {
    let f = fixture(&[("a.md", "正文")]);
    let mut app = f.app(None);
    tree_prompt(&mut app, 'a');
    app.paste("中👨‍👩‍👧‍👦e\u{301}尾");
    key(&mut app, KeyCode::Home);
    key(&mut app, KeyCode::Right);
    shift(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Backspace);
    key(&mut app, KeyCode::Delete); // Deletes the entire combining sequence.
    key(&mut app, KeyCode::Char('文'));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.note_path(), Some(f.path("中文尾.md").as_path()));
    assert_eq!(fs::read_to_string(f.path("a.md")).unwrap(), "正文");

    ctrl(&mut app, 'f');
    app.paste(&"中文".repeat(80));
    for code in [KeyCode::End, KeyCode::Home, KeyCode::Right] {
        key(&mut app, code);
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let cursor = terminal.get_cursor_position().unwrap();
        assert!(
            cursor.x < 39 && cursor.y < 11,
            "cursor must remain inside the input"
        );
        if code == KeyCode::Right {
            assert_eq!(cursor.x, 12, "one Chinese grapheme occupies two cells");
        }
    }
}

#[test]
fn full_text_search_reacts_to_undo_and_survives_switching_prompt_type() {
    let f = fixture(&[("a.md", "第一词"), ("b.md", "第二词")]);
    let mut app = f.app(None);
    search_notes(&mut app);
    app.paste("第一词");
    wait_for(&mut app, |a| !a.prompt_choices().is_empty());
    assert!(app.prompt_choices()[0].starts_with("a.md:"));
    ctrl(&mut app, 'a');
    app.paste("第二词");
    wait_for(&mut app, |a| !a.prompt_choices().is_empty());
    assert!(app.prompt_choices()[0].starts_with("b.md:"));
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Tab);
    ctrl(&mut app, 'z');
    wait_for(&mut app, |a| !a.prompt_choices().is_empty());
    assert!(app.prompt_choices()[0].starts_with("a.md:"));
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.note_text(), Some("第一词"));
}

#[test]
fn unbound_extended_shortcuts_never_type_letters_or_confirm_dialogs() {
    let f = fixture(&[("a.md", "正文")]);
    let mut app = f.app(None);
    for modifier in [KeyModifiers::SUPER, KeyModifiers::HYPER, KeyModifiers::META] {
        modified(&mut app, KeyCode::Char('q'), modifier);
        assert_eq!(app.note_text(), Some("正文"));
        assert!(!app.quit);
    }
    tree_prompt(&mut app, 'a');
    app.paste("名字");
    ctrl(&mut app, 'a');
    modified(&mut app, KeyCode::Char('k'), KeyModifiers::SUPER);
    key(&mut app, KeyCode::Enter);
    assert!(f.path("名字.md").exists());
    ctrl(&mut app, 'w');
    key(&mut app, KeyCode::Char('d'));
    assert!(app.has_dialog());
    modified(&mut app, KeyCode::Char('y'), KeyModifiers::SUPER);
    assert!(app.has_dialog());
    assert!(f.path("名字.md").exists());
}

#[test]
fn custom_command_bindings_are_saved_restored_and_apply_in_text_fields() {
    use linknote::app::keymap::{Action, Binding, Keymap, Scope};
    let f = fixture(&[("a.md", "正文")]);
    let keys_path = f.base.join("config/keys.json");
    fs::create_dir_all(keys_path.parent().unwrap()).unwrap();
    let mut map = Keymap::default();
    let event = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::SUPER);
    map.assign(&keys_path, Action::SelectAll, Some(event))
        .unwrap();
    let mut map = Keymap::load(&keys_path);
    assert_eq!(map.action(&event, Scope::Editor), Some(Action::SelectAll));
    assert!(map.assign(&keys_path, Action::Copy, Some(event)).is_err());
    assert!(
        Binding::parse("command+shift+z")
            .unwrap()
            .matches(&KeyEvent::new(
                KeyCode::Char('Z'),
                KeyModifiers::SUPER | KeyModifiers::SHIFT
            ))
    );
    assert!(!Binding::parse("ctrl+e").unwrap().matches(&event));
    let mut app = f.app(None);
    tree_prompt(&mut app, 'a');
    app.paste("原名字");
    app.key(event);
    app.paste("替换名字");
    key(&mut app, KeyCode::Enter);
    assert!(f.path("替换名字.md").exists());
    map.assign(&keys_path, Action::SelectAll, None).unwrap();
    assert_eq!(map.action(&event, Scope::Editor), None);
    if cfg!(target_os = "macos") {
        assert_eq!(
            map.action(
                &KeyEvent::new(KeyCode::Char('a'), KeyModifiers::SUPER),
                Scope::Editor
            ),
            Some(Action::SelectAll)
        );
    }
}

#[test]
fn image_viewer_is_modal_preserves_editor_and_opens_from_button_or_source() {
    let source = "# Image\n\n![one](large.png)\n\nTail\n";
    let f = fixture(&[("note.md", source)]);
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(1600, 800, |x, y| {
        image::Rgba([(x % 256) as u8, (y % 256) as u8, 130, 255])
    }))
    .save(f.path("large.png"))
    .unwrap();
    let mut app = f.app(Some("note.md"));
    let screen = draw(&mut app);
    let (row, line) = screen
        .iter()
        .enumerate()
        .find(|(_, l)| l.contains("🔍"))
        .unwrap();
    let col = UnicodeWidthStr::width(line.split_once("🔍").unwrap().0) as u16;
    let cursor = app.cursor();
    let focus = app.focus();
    mouse_at(&mut app, col, row as u16);
    assert!(app.has_dialog());
    assert!(draw(&mut app).join("\n").contains("图片查看"));
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        app.tick();
        let screen = draw(&mut app).join("\n");
        if !screen.contains("渲染中") {
            assert!(!screen.contains("无法查看"), "{screen}");
            break;
        }
        assert!(Instant::now() < deadline, "{screen}");
        std::thread::sleep(Duration::from_millis(10));
    }
    key(&mut app, KeyCode::Char('+'));
    assert!(draw(&mut app).join("\n").contains("125%"));
    for _ in 0..8 {
        key(&mut app, KeyCode::Char('+'));
    }
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Down);
    app.paste("must not edit");
    ctrl(&mut app, 'p');
    ctrl(&mut app, 'q');
    assert!(!app.quit);
    assert!(!app.has_prompt());
    key(&mut app, KeyCode::Char('o'));
    assert!(draw(&mut app).join("\n").contains("已交给系统应用打开"));
    draw_size(&mut app, 40, 10);
    key(&mut app, KeyCode::Char('0'));
    assert!(draw(&mut app).join("\n").contains("100%"));
    draw_size(&mut app, 12, 3);
    key(&mut app, KeyCode::Esc);
    assert!(!app.has_dialog());
    assert_eq!(app.cursor(), cursor);
    assert_eq!(app.focus(), focus);
    assert_eq!(app.note_text(), Some(source));
    assert_eq!(fs::read_to_string(f.path("note.md")).unwrap(), source);

    // A source selection stays intact, even while it is hidden by the viewer.
    draw(&mut app);
    ctrl(&mut app, 'f');
    app.paste("large.png");
    key(&mut app, KeyCode::Enter);
    let selected = app.selection();
    let at = app.cursor();
    key(&mut app, KeyCode::F(10));
    assert!(app.has_dialog());
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.selection(), selected);
    assert_eq!(app.cursor(), at);
}

#[test]
fn image_viewer_handles_missing_images_remote_permission_and_mermaid_source() {
    let source = "![missing](absent.png)\n\n![remote](https://example.invalid/private.png)\n\n```mermaid\nflowchart LR\n A --> B\n```\n";
    let f = fixture(&[("note.md", source)]);
    let mut app = f.app(Some("note.md"));
    key(&mut app, KeyCode::F(10));
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        app.tick();
        let screen = draw(&mut app).join("\n");
        if screen.contains("无法查看") {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    key(&mut app, KeyCode::Char('+'));
    assert!(!draw(&mut app).join("\n").contains("渲染中"));
    key(&mut app, KeyCode::Esc);
    ctrl(&mut app, 'f');
    app.paste("private.png");
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::F(10));
    assert!(!app.has_dialog());
    assert!(app.message().unwrap().contains("设置"));

    ctrl(&mut app, 'f');
    app.paste("A --> B");
    key(&mut app, KeyCode::Enter);
    let selected = app.selection();
    key(&mut app, KeyCode::F(10));
    assert!(app.has_dialog());
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        app.tick();
        let screen = draw(&mut app).join("\n");
        if !screen.contains("渲染中") {
            assert!(!screen.contains("无法查看"), "{screen}");
            break;
        }
        assert!(Instant::now() < deadline, "{screen}");
        std::thread::sleep(Duration::from_millis(20));
    }
    key(&mut app, KeyCode::Char('+'));
    key(&mut app, KeyCode::Char('o'));
    assert!(draw(&mut app).join("\n").contains("已交给系统应用打开"));
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.selection(), selected);
    assert_eq!(app.note_text(), Some(source));
}

#[test]
fn adjacent_images_have_distinct_zoom_buttons() {
    let f = fixture(&[(
        "note.md",
        "# Head\n\n![one](one.png) ![two](two.png)\n\nTail",
    )]);
    let mut app = f.app(Some("note.md"));
    let screen = draw(&mut app);
    let buttons: Vec<_> = screen
        .iter()
        .enumerate()
        .filter_map(|(row, line)| {
            line.split_once("🔍")
                .map(|(prefix, _)| (UnicodeWidthStr::width(prefix) as u16, row as u16))
        })
        .collect();
    assert_eq!(buttons.len(), 2);
    for ((column, row), name) in buttons.into_iter().zip(["one.png", "two.png"]) {
        mouse_at(&mut app, column, row);
        assert!(draw(&mut app)[0].contains(name));
        key(&mut app, KeyCode::Esc);
        draw(&mut app);
    }
}
