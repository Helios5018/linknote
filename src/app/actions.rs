use super::{App, Dialog, DiffLine, Focus, Prompt, PromptKind, keymap::Action};
use crate::{
    layout,
    links::{self, Target},
    search, tree,
};
use anyhow::{Context, Result, bail};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

impl App {
    pub(super) fn perform(&mut self, action: Action) -> Result<()> {
        match action {
            Action::Help => self.dialog = Some(Dialog::Help(0)),
            Action::Settings => self.open_settings(),
            Action::Quit => {
                if let Err(e) = self.flush() {
                    self.dialog = Some(Dialog::Conflict);
                    return Err(e);
                }
                self.write_session()?;
                self.quit = true;
            }
            Action::Save => {
                if self.note.as_ref().is_some_and(|n| n.conflict) {
                    self.dialog = Some(Dialog::Conflict);
                } else if let Some(n) = &mut self.note {
                    n.error = None;
                    n.save()?;
                    self.say("已保存（LinkNote 会自动保存，无需手动操作）".into());
                }
            }
            Action::Create | Action::NewNote => {
                self.start_prompt(PromptKind::Create(self.directory()))?
            }
            Action::NewFolder => self.start_prompt(PromptKind::NewFolder(self.directory()))?,
            Action::CopyPath => {
                if let Some(entry) = self.entries.get(self.tree_selected) {
                    let path = std::path::absolute(&entry.path)?
                        .to_string_lossy()
                        .into_owned();
                    self.say("已复制绝对路径".into());
                    self.copy_text(path);
                }
            }
            Action::Rename | Action::RenameTree | Action::Move | Action::MoveTree => {
                let path = if self.focus == Focus::Tree {
                    self.entries.get(self.tree_selected).map(|e| e.path.clone())
                } else {
                    self.note.as_ref().map(|n| n.path.clone())
                };
                if let Some(path) = path {
                    self.start_prompt(if matches!(action, Action::Rename | Action::RenameTree) {
                        PromptKind::Rename(path)
                    } else {
                        PromptKind::Move(path)
                    })?;
                }
            }
            Action::Delete => {
                if let Some(e) = self.entries.get(self.tree_selected) {
                    self.dialog = Some(Dialog::Delete(e.path.clone()));
                }
            }
            Action::OpenNote => self.start_prompt(PromptKind::Open)?,
            Action::SearchNotes => self.start_prompt(PromptKind::Search)?,
            Action::Find => self.start_prompt(PromptKind::Find)?,
            Action::FindNext => self.find_next(),
            Action::FollowLink => self.follow_link()?,
            Action::Back => self.back()?,
            Action::SwitchFocus => {
                self.focus = if self.focus == Focus::Tree {
                    Focus::Editor
                } else {
                    Focus::Tree
                };
                if self.focus == Focus::Tree {
                    self.sidebar = true;
                    self.tree_follow = true;
                }
            }
            Action::ToggleSidebar => {
                self.sidebar = !self.sidebar_visible();
                self.focus = if self.sidebar && self.body_rect.width < 64 {
                    Focus::Tree
                } else {
                    Focus::Editor
                };
                self.resizing = None;
                self.dragging = false;
                self.desired_column = None;
                self.reveal = true;
            }
            Action::NarrowSidebar | Action::WidenSidebar => {
                let delta = if action == Action::NarrowSidebar {
                    -2
                } else {
                    2
                };
                self.resize_sidebar(i32::from(self.sidebar_columns(self.body_rect.width)) + delta);
                self.write_session()?;
            }
            Action::ViewImage => self.open_image(None)?,
            Action::ReloadImages => {
                self.images.refresh();
                self.layout_key = None;
                self.reveal = true;
            }
            Action::LoadRemoteImages => {
                let Some(path) = self.note.as_ref().map(|n| n.path.clone()) else {
                    return Ok(());
                };
                if self.session.remote_images.remove(&path) {
                    self.say("本篇不再加载网络图片".into());
                } else {
                    self.session.remote_images.insert(path);
                    self.say("正在加载本篇的网络图片；之后打开本篇也会自动加载".into());
                }
                self.sync_remote_images();
                self.layout_key = None;
                self.write_session()?;
            }
            Action::Resolve => {
                if self
                    .note
                    .as_ref()
                    .is_some_and(|n| n.conflict || n.error.is_some())
                {
                    self.dialog = Some(Dialog::Conflict);
                } else if let Some(n) = &mut self.note {
                    n.error = None;
                    n.save()?;
                    self.say("已重试保存".into());
                }
            }
            Action::Undo | Action::Redo | Action::SelectAll => {
                if let Some(n) = &mut self.note {
                    let (before, dirty) = (n.doc.revision, n.dirty());
                    match action {
                        Action::Undo => n.doc.undo(),
                        Action::Redo => n.doc.redo(),
                        _ => n.doc.select_all(),
                    }
                    if n.doc.revision != before {
                        n.edited(dirty);
                    }
                    self.invalidate();
                }
            }
            Action::Copy | Action::Cut => {
                if let Some(text) = self
                    .note
                    .as_ref()
                    .and_then(|n| n.doc.selected_text())
                    .map(str::to_owned)
                {
                    self.copy_text(text);
                    if action == Action::Cut {
                        self.paste("");
                    }
                }
            }
            Action::Paste => {
                let text = self.clipboard_text();
                if !text.is_empty() {
                    self.paste(&text);
                }
            }
        }
        Ok(())
    }

    pub(super) fn copy_text(&mut self, text: String) {
        self.clipboard = text.clone();
        if !self.options.internal_clipboard
            && let Err(e) = arboard::Clipboard::new().and_then(|mut c| c.set_text(text))
        {
            self.say(format!("已复制到应用剪贴板（系统剪贴板不可用：{e}）"));
        }
    }

    pub(super) fn clipboard_text(&self) -> String {
        if self.options.internal_clipboard {
            return self.clipboard.clone();
        }
        arboard::Clipboard::new()
            .and_then(|mut c| c.get_text())
            .unwrap_or_else(|_| self.clipboard.clone())
    }

    pub(super) fn start_prompt(&mut self, kind: PromptKind) -> Result<()> {
        let choices = match &kind {
            PromptKind::Open => tree::all_notes(&self.root)?,
            PromptKind::Move(path) => tree::all_folders(&self.root)?
                .into_iter()
                .filter(|d| !d.starts_with(path) && Some(d.as_path()) != path.parent())
                .collect(),
            _ => vec![],
        };
        let kind_is_find = matches!(kind, PromptKind::Find);
        let text = match &kind {
            PromptKind::Rename(path) => path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            PromptKind::Find => self.find.clone(),
            _ => String::new(),
        };
        self.prompt = Some(Prompt {
            kind,
            input: super::Prompt::text_field(text, kind_is_find),
            replacement: None,
            editing_replace: false,
            selected: 0,
            choices,
            hits: vec![],
            truncated: false,
            searching: false,
            count: 0,
        });
        self.count_matches();
        Ok(())
    }
    /// Refresh the match count shown in the find prompt.
    pub(super) fn count_matches(&mut self) {
        if let (Some(p), Some(n)) = (&mut self.prompt, &self.note)
            && matches!(p.kind, PromptKind::Find)
        {
            p.count = if p.input.text.is_empty() {
                0
            } else {
                search::find_all(&n.doc.text, &p.input.text).len()
            };
        }
    }
    pub(super) fn filtered(&self, p: &Prompt) -> Vec<PathBuf> {
        let query = p.input.text.to_lowercase();
        p.choices
            .iter()
            .filter(|path| self.relative(path).to_lowercase().contains(&query))
            .cloned()
            .collect()
    }
    /// Start a background full-text search for the prompt's text.
    pub(super) fn request_search(&mut self) {
        let current = self
            .note
            .as_ref()
            .map(|n| (n.path.clone(), n.doc.text.clone()));
        if let Some(p) = &mut self.prompt {
            p.hits.clear();
            p.selected = 0;
            p.truncated = false;
            if p.input.text.trim().is_empty() {
                p.searching = false;
                self.search.cancel();
            } else {
                p.searching = true;
                self.search.request(&self.root, &p.input.text, current);
            }
        }
    }

    pub(super) fn submit(&mut self, p: &Prompt) -> Result<()> {
        match &p.kind {
            PromptKind::Create(parent) => {
                let input = p.input.text.trim();
                if let Some(name) = input.strip_suffix('/') {
                    self.create_folder(parent, name)?;
                } else {
                    let name = tree::validated_name(input, false)?;
                    let name = if Path::new(&name).extension().is_none() {
                        format!("{name}.md")
                    } else if tree::markdown(Path::new(&name)) {
                        name
                    } else {
                        bail!("只支持新建 Markdown；请去掉后缀或使用 .md，文件夹请以 / 结尾");
                    };
                    let path = parent.join(name);
                    self.leave_note()?;
                    fs::OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(&path)
                        .context("新建失败，文件可能已存在")?
                        .sync_all()?;
                    self.expanded.insert(parent.clone());
                    self.open(path)?;
                }
            }
            PromptKind::NewNote(parent) => {
                self.leave_note()?;
                let path = parent.join(tree::validated_name(&p.input.text, true)?);
                let mut file = fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&path)
                    .context("新建失败，文件可能已存在")?;
                file.write_all(b"")?;
                file.sync_all()?;
                self.expanded.insert(parent.clone());
                self.open(path)?;
            }
            PromptKind::NewFolder(parent) => {
                self.create_folder(
                    parent,
                    p.input
                        .text
                        .trim()
                        .strip_suffix('/')
                        .unwrap_or(p.input.text.trim()),
                )?;
            }
            PromptKind::Rename(old) => {
                let new = old
                    .parent()
                    .unwrap()
                    .join(tree::validated_name(&p.input.text, !old.is_dir())?);
                if new == *old {
                    return Ok(());
                }
                // On a case-insensitive file system `a.md` and `A.md` are the same file.
                if new.exists() && !tree::same_file(old, &new) {
                    bail!("同名文件或文件夹已存在");
                }
                self.move_path(old, &new).context("重命名失败")?;
            }
            PromptKind::Move(old) => {
                let choices = self.filtered(p);
                let folder = choices
                    .get(p.selected.min(choices.len().saturating_sub(1)))
                    .context("没有匹配的文件夹")?;
                let new = folder.join(old.file_name().context("无法移动笔记根目录")?);
                if new.exists() {
                    bail!("目标文件夹中已有同名项目");
                }
                self.move_path(old, &new).context("移动失败")?;
                self.expanded.insert(folder.clone());
                self.refresh_tree()?;
                self.select_in_tree(&new);
                self.say(format!("已移动到 {}", self.relative(folder)));
            }
            PromptKind::Find => {
                self.find = p.input.text.clone();
                self.find_next();
            }
            PromptKind::Open => {
                let choices = self.filtered(p);
                let path = choices
                    .get(p.selected.min(choices.len().saturating_sub(1)))
                    .context("没有匹配的 Markdown 文件")?;
                self.open(path.clone())?;
            }
            PromptKind::Search => {
                let hit = p
                    .hits
                    .get(p.selected.min(p.hits.len().saturating_sub(1)))
                    .context(if p.searching {
                        "仍在搜索"
                    } else {
                        "没有匹配的内容"
                    })?
                    .clone();
                self.open(hit.path.clone())?;
                self.find = p.input.text.clone();
                if let Some(n) = &mut self.note {
                    let start = n.doc.boundary(hit.range.start.min(n.doc.text.len()));
                    n.doc.set_cursor(start, false);
                    n.doc.set_cursor(hit.range.end.min(n.doc.text.len()), true);
                }
                self.invalidate();
            }
        }
        Ok(())
    }

    fn create_folder(&mut self, parent: &Path, name: &str) -> Result<()> {
        let path = parent.join(tree::validated_name(name, false)?);
        fs::create_dir(&path).context("新建文件夹失败")?;
        self.expanded.insert(parent.to_path_buf());
        self.refresh_tree()?;
        self.select_in_tree(&path);
        self.focus = Focus::Tree;
        Ok(())
    }

    /// Rename or move a file or folder and keep the open note, tree and session in step.
    fn move_path(&mut self, old: &Path, new: &Path) -> Result<()> {
        if self.note.as_ref().is_some_and(|n| n.path.starts_with(old)) {
            self.leave_note()?;
        }
        fs::rename(old, new)?;
        let moved = |path: &Path| -> Option<PathBuf> {
            let suffix = path.strip_prefix(old).ok()?;
            Some(if suffix.as_os_str().is_empty() {
                new.to_path_buf()
            } else {
                new.join(suffix)
            })
        };
        if let Some(n) = &mut self.note
            && let Some(path) = moved(&n.path)
        {
            n.relocate(path)?;
        }
        self.expanded = self
            .expanded
            .iter()
            .map(|p| moved(p).unwrap_or_else(|| p.clone()))
            .collect();
        self.session.cursors = std::mem::take(&mut self.session.cursors)
            .into_iter()
            .map(|(p, c)| (moved(&p).unwrap_or(p), c))
            .collect();
        self.session.remote_images = std::mem::take(&mut self.session.remote_images)
            .into_iter()
            .map(|p| moved(&p).unwrap_or(p))
            .collect();
        for (path, _) in &mut self.history {
            if let Some(p) = moved(path) {
                *path = p;
            }
        }
        self.refresh_tree()?;
        self.select_in_tree(new);
        self.write_session()?;
        Ok(())
    }

    pub(super) fn delete(&mut self, path: &Path) -> Result<()> {
        if self.note.as_ref().is_some_and(|n| n.path.starts_with(path)) {
            self.flush().context("请先处理当前笔记的保存问题，再删除")?;
            if let Some(n) = self.note.take() {
                n.clear_journal()?;
            }
            self.layout = None;
            self.layout_key = None;
        }
        match &self.options.trash_dir {
            Some(dir) => {
                fs::create_dir_all(dir)?;
                let name = path.file_name().context("无法删除笔记根目录")?;
                let mut target = dir.join(name);
                let mut n = 1;
                while target.exists() {
                    target = dir.join(format!("{}-{n}", name.to_string_lossy()));
                    n += 1;
                }
                fs::rename(path, target)?;
            }
            None => {
                #[allow(unused_mut)]
                let mut trash = trash::TrashContext::default();
                #[cfg(target_os = "macos")]
                {
                    use trash::macos::{DeleteMethod, TrashContextExtMacos};
                    trash.set_delete_method(DeleteMethod::NsFileManager);
                }
                trash.delete(path).context("无法移到废纸篓")?;
            }
        }
        self.history.retain(|(p, _)| !p.starts_with(path));
        self.refresh_tree()?;
        self.write_session()?;
        self.say(format!("已将「{}」移到废纸篓", self.relative(path)));
        Ok(())
    }

    pub(super) fn find_next(&mut self) {
        if self.find.is_empty() {
            return;
        }
        if let Some(n) = &mut self.note {
            let from = n.doc.selection().map_or(n.doc.cursor, |r| r.end);
            if let Some(r) = search::find_next(&n.doc.text, &self.find, from) {
                n.doc.set_cursor(r.start, false);
                n.doc.set_cursor(r.end, true);
                self.focus = Focus::Editor;
                self.invalidate();
            } else {
                self.say("没有找到匹配文本".into());
            }
        }
    }
    /// Replace the selected match, then select the next one.
    pub(super) fn replace_next(&mut self) {
        let Some(n) = &mut self.note else {
            return;
        };
        if let Some(r) = n.doc.selection()
            && search::is_match(&n.doc.text, &self.find, r.clone())
        {
            let dirty = n.dirty();
            n.doc.insert(&self.replace.clone());
            n.edited(dirty);
        }
        self.find_next();
        self.count_matches();
    }
    pub(super) fn replace_all(&mut self) -> usize {
        let Some(n) = &mut self.note else {
            return 0;
        };
        let (text, count) = search::replace_all(&n.doc.text, &self.find, &self.replace);
        if count > 0 {
            let dirty = n.dirty();
            n.doc.replace_all(text);
            n.edited(dirty);
            self.invalidate();
        }
        count
    }

    fn follow_link(&mut self) -> Result<()> {
        let Some(n) = &self.note else {
            return Ok(());
        };
        let cursor = n.doc.cursor;
        let (dest, wiki) = match layout::link_at(&n.doc.text, cursor) {
            Some(link) => link,
            None => match links::url_at(&n.doc.text, cursor) {
                Some(url) => (url, false),
                None => bail!("光标处没有链接"),
            },
        };
        let from = (n.path.clone(), cursor);
        match links::resolve(&self.root, &n.path, &dest, wiki) {
            Target::Heading(anchor) => {
                self.history.push(from);
                self.jump_to_heading(&anchor)?;
            }
            Target::Note(path, heading) => {
                self.open(path)?;
                self.history.push(from);
                if let Some(anchor) = heading {
                    self.jump_to_heading(&anchor)?;
                }
            }
            Target::Missing(path) => {
                let parent = path.parent().context("链接目标无效")?.to_path_buf();
                if !parent.is_dir() {
                    bail!("链接指向的文件夹不存在：{}", self.relative(&parent));
                }
                self.start_prompt(PromptKind::NewNote(parent))?;
                if let Some(p) = &mut self.prompt {
                    let text = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    p.input = super::Prompt::text_field(text, false);
                }
                self.say("链接的笔记还不存在，按 Enter 创建".into());
            }
            Target::Folder(path) => {
                for ancestor in path.ancestors().take_while(|p| *p != self.root) {
                    self.expanded.insert(ancestor.to_path_buf());
                }
                self.refresh_tree()?;
                self.select_in_tree(&path);
                self.focus = Focus::Tree;
                self.sidebar = true;
            }
            Target::External(target) => {
                links::open_external(&target, self.options.opener.as_deref())?;
                self.say(format!("已用系统程序打开 {target}"));
            }
            Target::Outside => bail!("链接指向笔记目录之外，未打开"),
        }
        Ok(())
    }
    fn jump_to_heading(&mut self, anchor: &str) -> Result<()> {
        let n = self.note.as_mut().context("没有打开的笔记")?;
        let wanted = links::slug(anchor);
        let (_, at) = layout::headings(&n.doc.text)
            .into_iter()
            .find(|(text, _)| links::slug(text) == wanted)
            .with_context(|| format!("找不到标题 #{anchor}"))?;
        n.doc.set_cursor(at, false);
        self.focus = Focus::Editor;
        self.invalidate();
        Ok(())
    }
    fn back(&mut self) -> Result<()> {
        let (path, cursor) = self.history.pop().context("没有可以返回的位置")?;
        self.open(path)?;
        if let Some(n) = &mut self.note {
            n.doc.set_cursor(cursor.min(n.doc.text.len()), false);
        }
        self.invalidate();
        Ok(())
    }

    pub(super) fn show_diff(&mut self) -> Result<()> {
        let n = self.note.as_ref().context("没有打开的笔记")?;
        let disk = n.disk_text().unwrap_or_default();
        let diff = similar::TextDiff::from_lines(&disk, &n.doc.text);
        let mut lines = vec![];
        for group in diff.grouped_ops(2) {
            if !lines.is_empty() {
                lines.push(DiffLine {
                    tag: similar::ChangeTag::Equal,
                    text: "⋯".into(),
                });
            }
            for op in group {
                for change in diff.iter_changes(&op) {
                    lines.push(DiffLine {
                        tag: change.tag(),
                        text: change.value().trim_end_matches('\n').to_owned(),
                    });
                }
            }
        }
        if lines.is_empty() {
            lines.push(DiffLine {
                tag: similar::ChangeTag::Equal,
                text: "两个版本内容相同（可能只有换行符不同）".into(),
            });
        }
        self.dialog = Some(Dialog::Diff(lines, 0));
        Ok(())
    }
    /// Conflict choice: keep the disk version. The local text stays in undo history.
    pub(super) fn use_disk_version(&mut self) -> Result<()> {
        let n = self.note.as_mut().context("没有打开的笔记")?;
        n.use_disk()?;
        self.invalidate();
        self.say("已改用磁盘版本；Ctrl+Z 可找回本地内容".into());
        Ok(())
    }
    /// Conflict choice made explicitly: write the local version over the disk.
    pub(super) fn overwrite_with_local(&mut self) -> Result<()> {
        let n = self.note.as_mut().context("没有打开的笔记")?;
        n.overwrite_disk()?;
        self.say("已用本地版本覆盖磁盘文件".into());
        Ok(())
    }
    pub(super) fn keep_both(&mut self) -> Result<()> {
        let n = self.note.as_mut().context("没有打开的笔记")?;
        let copy = n.keep_both()?;
        // The new copy is durable, so replace the conflicted buffer without flushing it.
        self.remember();
        self.note = None;
        self.open(copy)?;
        self.say("本地内容已另存为 .local-N.md，外部版本保持原样".into());
        Ok(())
    }
}
