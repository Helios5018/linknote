//! Application state. Input is translated into actions (`input.rs`, `keymap.rs`),
//! actions change this state (`actions.rs`), and `view.rs` draws it.
mod actions;
mod image_viewer;
mod input;
pub mod keymap;
mod settings_panel;
mod theme;
mod view;

use crate::{
    document::Document,
    images::Images,
    layout::Layout,
    search,
    settings::{Settings, Store},
    storage::{self, Draft, DraftSetAside, Note},
    tree::{self, Entry},
};
use anyhow::{Context, Result};
use directories::ProjectDirs;
use keymap::Keymap;
use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Where LinkNote keeps its own files, and how it reaches the system.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Session, cursor and recovery files. Default: the platform data directory.
    pub state_dir: Option<PathBuf>,
    /// `keys.json` and `settings.json`. Default: the platform config directory.
    pub config_dir: Option<PathBuf>,
    /// Move deleted items here instead of the system Trash.
    pub trash_dir: Option<PathBuf>,
    /// Program used to open URLs and non-Markdown files.
    pub opener: Option<String>,
    /// Use only the application clipboard, for headless sessions and isolated tests.
    pub internal_clipboard: bool,
}

impl Options {
    pub fn from_env() -> Self {
        let var = |name| std::env::var_os(name).filter(|v| !v.is_empty());
        Self {
            state_dir: var("LINKNOTE_STATE_DIR").map(PathBuf::from),
            config_dir: var("LINKNOTE_CONFIG_DIR").map(PathBuf::from),
            trash_dir: var("LINKNOTE_TRASH_DIR").map(PathBuf::from),
            opener: var("LINKNOTE_OPENER").map(|v| v.to_string_lossy().into_owned()),
            internal_clipboard: std::env::var("LINKNOTE_CLIPBOARD").is_ok_and(|v| v == "internal"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Focus {
    Tree,
    Editor,
}

enum PromptKind {
    Create(PathBuf),
    NewNote(PathBuf),
    NewFolder(PathBuf),
    Rename(PathBuf),
    Move(PathBuf),
    Find,
    Open,
    Search,
}

struct Prompt {
    kind: PromptKind,
    input: Document,
    /// The replacement in the find prompt, once Tab has opened it.
    replacement: Option<Document>,
    editing_replace: bool,
    selected: usize,
    choices: Vec<PathBuf>,
    hits: Vec<search::Hit>,
    truncated: bool,
    searching: bool,
    /// Matches of the find text in the open note.
    count: usize,
}

impl Prompt {
    fn field(&self) -> &Document {
        if self.editing_replace {
            self.replacement
                .as_ref()
                .expect("replacement field is open")
        } else {
            &self.input
        }
    }

    fn field_mut(&mut self) -> &mut Document {
        if self.editing_replace {
            self.replacement
                .as_mut()
                .expect("replacement field is open")
        } else {
            &mut self.input
        }
    }

    fn text_field(text: String, selected: bool) -> Document {
        let mut doc = Document::new(text);
        doc.set_cursor(doc.text.len(), false);
        if selected {
            doc.select_all();
        }
        doc
    }
}

struct DiffLine {
    tag: similar::ChangeTag,
    text: String,
}

enum Dialog {
    Settings(settings_panel::Panel),
    Help(usize),
    Conflict,
    Diff(Vec<DiffLine>, usize),
    Recovery(Draft),
    Delete(PathBuf),
}

#[derive(Default, Serialize, Deserialize)]
struct Session {
    last: Option<PathBuf>,
    cursors: HashMap<PathBuf, usize>,
    #[serde(default)]
    sidebar_width: Option<u16>,
    /// Notes whose network images the user chose to load.
    #[serde(default)]
    remote_images: HashSet<PathBuf>,
}

pub struct App {
    root: PathBuf,
    options: Options,
    state_dir: PathBuf,
    session_path: PathBuf,
    session: Session,
    keymap: Keymap,
    keys_path: PathBuf,
    settings: Settings,
    settings_store: Store,
    settings_button: Rect,
    sidebar_button: Rect,
    images: Images,
    viewer: Option<image_viewer::Viewer>,
    viewer_exports: Vec<std::sync::Arc<tempfile::TempDir>>,
    image_buttons: Vec<(Rect, String)>,
    entries: Vec<Entry>,
    expanded: HashSet<PathBuf>,
    /// Notes (other than the open one) with an unresolved recovery draft.
    drafts: HashSet<PathBuf>,
    tree_selected: usize,
    tree_scroll: usize,
    /// Scroll the tree to the selection on the next draw.
    tree_follow: bool,
    note: Option<Note>,
    focus: Focus,
    sidebar: bool,
    prompt: Option<Prompt>,
    dialog: Option<Dialog>,
    message: Option<(String, Instant)>,
    clipboard: String,
    last_poll: Instant,
    last_tree: Instant,
    last_journal: Instant,
    editor_rect: Rect,
    tree_rect: Rect,
    tree_area: Rect,
    tree_click: Option<(PathBuf, Instant)>,
    body_rect: Rect,
    divider_rect: Rect,
    resizing: Option<i32>,
    layout: Option<Layout>,
    layout_key: Option<(u64, usize, Option<usize>, u16)>,
    desired_column: Option<usize>,
    find: String,
    replace: String,
    dragging: bool,
    /// Notes visited before following links, for Back.
    history: Vec<(PathBuf, usize)>,
    search: search::Worker,
    pub reveal: bool,
    pub quit: bool,
}

impl App {
    pub fn new(root: PathBuf, initial: Option<PathBuf>) -> Result<Self> {
        Self::with_options(root, initial, Options::from_env())
    }
    pub fn with_options(root: PathBuf, initial: Option<PathBuf>, options: Options) -> Result<Self> {
        let dirs = ProjectDirs::from("", "", "linknote");
        let state_dir = options
            .state_dir
            .clone()
            .or_else(|| dirs.as_ref().map(|p| p.data_local_dir().to_path_buf()))
            .context("无法定位应用数据目录，请设置 LINKNOTE_STATE_DIR")?;
        let config_dir = options
            .config_dir
            .clone()
            .or_else(|| dirs.as_ref().map(|p| p.config_dir().to_path_buf()))
            .unwrap_or_else(|| state_dir.clone());
        fs::create_dir_all(state_dir.join("recovery"))?;
        let session_path = storage::recovery_path(&state_dir, &root).with_extension("session.json");
        let session: Session = fs::read(&session_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let entries = tree::scan(&root, &HashSet::new())?;
        let first = initial
            .or_else(|| {
                session
                    .last
                    .clone()
                    .filter(|p| p.starts_with(&root) && p.is_file())
            })
            .or_else(|| {
                entries
                    .iter()
                    .find(|e| !e.directory)
                    .map(|e| e.path.clone())
            });
        let keymap = Keymap::load(&config_dir.join("keys.json"));
        let (settings_store, settings) = Store::load(config_dir.join("settings.json"));
        let mut images = Images::new(ratatui_image::picker::Picker::halfblocks());
        images.configure(&settings);
        let sidebar = settings.sidebar_visible;
        let now = Instant::now();
        let mut app = Self {
            root,
            options,
            state_dir,
            session_path,
            session,
            keymap,
            keys_path: config_dir.join("keys.json"),
            settings,
            settings_store,
            settings_button: Rect::default(),
            sidebar_button: Rect::default(),
            images,
            viewer: None,
            viewer_exports: vec![],
            image_buttons: vec![],
            entries,
            expanded: HashSet::new(),
            drafts: HashSet::new(),
            tree_selected: 0,
            tree_scroll: 0,
            tree_follow: true,
            note: None,
            focus: Focus::Tree,
            sidebar,
            prompt: None,
            dialog: None,
            message: None,
            clipboard: String::new(),
            last_poll: now,
            last_tree: now,
            last_journal: now,
            editor_rect: Rect::default(),
            tree_rect: Rect::default(),
            tree_area: Rect::default(),
            tree_click: None,
            body_rect: Rect::default(),
            divider_rect: Rect::default(),
            resizing: None,
            layout: None,
            layout_key: None,
            desired_column: None,
            find: String::new(),
            replace: String::new(),
            dragging: false,
            history: vec![],
            search: search::Worker::new(),
            reveal: true,
            quit: false,
        };
        if let Some(warning) = app.keymap.warning.take() {
            app.say(warning);
        }
        if let Some(warning) = app.settings_store.warning.clone() {
            app.say(warning);
        }
        if let Some(path) = first
            && let Err(e) = app.open(path)
        {
            app.say(format!("打开失败：{e:#}"));
        }
        Ok(app)
    }

    // --- Read-only accessors, used by the binary and by tests. ---

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn note_path(&self) -> Option<&Path> {
        self.note.as_ref().map(|n| n.path.as_path())
    }
    pub fn note_text(&self) -> Option<&str> {
        self.note.as_ref().map(|n| n.doc.text.as_str())
    }
    pub fn cursor(&self) -> Option<usize> {
        self.note.as_ref().map(|n| n.doc.cursor)
    }
    pub fn selection(&self) -> Option<std::ops::Range<usize>> {
        self.note.as_ref().and_then(|n| n.doc.selection())
    }
    pub fn focus(&self) -> Focus {
        self.focus
    }
    pub fn message(&self) -> Option<&str> {
        self.message.as_ref().map(|(m, _)| m.as_str())
    }
    pub fn has_dialog(&self) -> bool {
        self.dialog.is_some() || self.viewer.is_some()
    }
    pub fn has_prompt(&self) -> bool {
        self.prompt.is_some()
    }
    pub fn conflict(&self) -> bool {
        self.note.as_ref().is_some_and(|n| n.conflict)
    }
    pub fn tree_paths(&self) -> Vec<PathBuf> {
        self.entries.iter().map(|e| e.path.clone()).collect()
    }
    /// Results currently listed by the open or search prompt.
    pub fn prompt_choices(&self) -> Vec<String> {
        let Some(p) = &self.prompt else {
            return vec![];
        };
        match p.kind {
            PromptKind::Search => p
                .hits
                .iter()
                .map(|h| format!("{}:{} {}", self.relative(&h.path), h.line, h.snippet))
                .collect(),
            _ => self
                .filtered(p)
                .iter()
                .map(|path| self.relative(path))
                .collect(),
        }
    }

    fn relative(&self, path: &Path) -> String {
        let rel = path.strip_prefix(&self.root).unwrap_or(path);
        if rel.as_os_str().is_empty() {
            "（笔记根目录）".into()
        } else {
            rel.display().to_string()
        }
    }
    fn say(&mut self, text: String) {
        self.message = Some((text, Instant::now()));
    }
    pub fn configure_images(&mut self, picker: ratatui_image::picker::Picker) {
        self.images.configure_picker(picker);
        self.sync_remote_images();
        self.layout_key = None;
    }
    fn sync_remote_images(&mut self) {
        let allowed = self
            .note
            .as_ref()
            .is_some_and(|n| self.session.remote_images.contains(&n.path));
        self.images.set_remote(allowed);
    }
    fn invalidate(&mut self) {
        self.layout_key = None;
        self.reveal = true;
        self.desired_column = None;
    }
    fn remember(&mut self) {
        if let Some(n) = &self.note {
            self.session.last = Some(n.path.clone());
            self.session.cursors.insert(n.path.clone(), n.doc.cursor);
        }
    }
    fn write_session(&mut self) -> Result<()> {
        self.remember();
        let root = self.root.clone();
        self.session
            .cursors
            .retain(|p, _| p.starts_with(&root) && p.is_file());
        self.session
            .remote_images
            .retain(|p| p.starts_with(&root) && p.is_file());
        storage::atomic_write(&self.session_path, &serde_json::to_vec(&self.session)?)
    }
    fn flush(&mut self) -> Result<()> {
        if let Some(n) = &mut self.note {
            n.save()?;
        }
        Ok(())
    }
    /// Save before switching away. A note with an unresolved conflict or save error
    /// is kept in its recovery draft instead, so the user is not stuck on it.
    fn leave_note(&mut self) -> Result<()> {
        let Some(n) = &mut self.note else {
            return Ok(());
        };
        if let Err(e) = n.save() {
            n.journal()
                .with_context(|| format!("{e:#}；恢复草稿也无法写入，暂不能离开"))?;
            let name = n
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            self.drafts.insert(n.path.clone());
            self.say(format!(
                "「{name}」的保存问题尚未解决，内容保留在恢复草稿中；重新打开它可继续处理"
            ));
        }
        Ok(())
    }
    fn sidebar_visible(&self) -> bool {
        self.sidebar && (self.body_rect.width >= 64 || self.focus == Focus::Tree)
    }
    fn sidebar_columns(&self, width: u16) -> u16 {
        self.session
            .sidebar_width
            .unwrap_or((width / 4).clamp(20, 32))
            .clamp(20, width.saturating_sub(32).max(20))
    }
    fn resize_sidebar(&mut self, columns: i32) {
        if self.body_rect.width < 64 {
            return;
        }
        self.session.sidebar_width =
            Some(columns.clamp(20, i32::from(self.body_rect.width - 32)) as u16);
        self.reveal = true;
        self.desired_column = None;
    }
    fn open(&mut self, path: PathBuf) -> Result<()> {
        if self.note.as_ref().is_some_and(|n| n.path == path) {
            self.focus = Focus::Editor;
            return Ok(());
        }
        self.leave_note()?;
        let mut next = Note::open(path.clone(), &self.state_dir.join("recovery"))?;
        if let Some(&cursor) = self.session.cursors.get(&path) {
            next.doc.set_cursor(cursor, false);
        }
        // A corrupt draft has been moved aside; open the disk version and say where.
        let (draft, notice) = match next.draft() {
            Ok(draft) => (draft, None),
            Err(e) => match e.downcast_ref::<DraftSetAside>() {
                Some(aside) => (None, Some(format!("{aside}，已打开磁盘版本"))),
                None => return Err(e),
            },
        };
        self.remember();
        self.note = Some(next);
        self.layout = None;
        self.drafts.remove(&path);
        self.focus = Focus::Editor;
        for ancestor in path
            .ancestors()
            .skip(1)
            .take_while(|p| p.starts_with(&self.root) && *p != self.root)
        {
            self.expanded.insert(ancestor.to_path_buf());
        }
        self.refresh_tree()?;
        self.select_in_tree(&path);
        self.dialog = draft.map(Dialog::Recovery);
        self.sync_remote_images();
        self.invalidate();
        self.write_session()?;
        if let Some(notice) = notice {
            self.say(notice);
        }
        Ok(())
    }
    fn select_in_tree(&mut self, path: &Path) {
        if let Some(i) = self.entries.iter().position(|e| e.path == path) {
            self.tree_selected = i;
            self.tree_follow = true;
        }
    }
    fn refresh_tree(&mut self) -> Result<()> {
        let selected = self.entries.get(self.tree_selected).map(|e| e.path.clone());
        self.entries = tree::scan(&self.root, &self.expanded)?;
        self.tree_selected = selected
            .and_then(|p| self.entries.iter().position(|e| e.path == p))
            .unwrap_or(self.tree_selected)
            .min(self.entries.len().saturating_sub(1));
        let recovery = self.state_dir.join("recovery");
        let current = self.note.as_ref().map(|n| n.path.clone());
        self.drafts = self
            .entries
            .iter()
            .filter(|e| !e.directory && Some(&e.path) != current.as_ref())
            .filter(|e| storage::recovery_path(&recovery, &e.path).exists())
            .map(|e| e.path.clone())
            .collect();
        Ok(())
    }
    fn directory(&self) -> PathBuf {
        if self.focus == Focus::Tree
            && let Some(e) = self.entries.get(self.tree_selected)
        {
            return if e.directory {
                e.path.clone()
            } else {
                e.path.parent().unwrap().to_path_buf()
            };
        }
        self.note
            .as_ref()
            .map(|n| n.path.parent().unwrap().to_path_buf())
            .unwrap_or_else(|| self.root.clone())
    }
    fn build_layout(&mut self) {
        let Some(n) = &self.note else {
            return;
        };
        let key = (
            n.doc.revision,
            n.doc.cursor,
            n.doc.anchor,
            self.editor_rect.width,
        );
        if self.layout_key == Some(key) {
            return;
        }
        let width = self.editor_rect.width as usize;
        let mut layout = Layout::update(self.layout.take(), &n.doc, width, &self.images.heights());
        if self.images.sync_urls(
            &n.path,
            n.doc.revision,
            &layout.image_urls,
            self.editor_rect.width.saturating_sub(4),
        ) {
            // Newly seen images get a placeholder row right away.
            layout = Layout::update(Some(layout), &n.doc, width, &self.images.heights());
        }
        self.layout = Some(layout);
        self.layout_key = Some(key);
    }
    pub fn tick(&mut self) {
        if let Some(viewer) = &mut self.viewer {
            viewer.poll();
        }
        if self.images.poll() {
            self.layout_key = None;
            if self.viewer.is_none() {
                self.reveal = true;
            }
        }
        if let Some(results) = self.search.poll()
            && let Some(p) = &mut self.prompt
            && matches!(p.kind, PromptKind::Search)
            && p.input.text == results.query
        {
            p.hits = results.hits;
            p.truncated = results.truncated;
            p.searching = false;
            p.selected = p.selected.min(p.hits.len().saturating_sub(1));
        }
        let recovery_pending = matches!(self.dialog, Some(Dialog::Recovery(_)));
        if !recovery_pending && let Some(n) = &mut self.note {
            if self.last_journal.elapsed() >= n.journal_interval() {
                // A failure is kept in `journal_error` and shown; saving goes on.
                let _ = n.journal();
                self.last_journal = Instant::now();
            }
            if n.save_due()
                && let Err(e) = n.save()
            {
                n.error = Some(format!("保存失败：{e:#}"));
            }
            if self.last_poll.elapsed() >= Duration::from_secs(1) {
                if let Err(e) = n.check_external() {
                    n.error = Some(format!("磁盘检查：{e:#}"));
                }
                self.last_poll = Instant::now();
            }
        }
        if self.last_tree.elapsed() >= Duration::from_secs(2) {
            if let Err(e) = self.refresh_tree() {
                self.say(format!("目录刷新失败：{e:#}"));
            }
            self.last_tree = Instant::now();
        }
    }
    pub fn emergency_journal(&mut self) {
        if let Some(n) = &mut self.note {
            let _ = n.journal();
        }
    }
}
