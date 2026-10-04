use super::{
    App, Dialog, Focus, Prompt, PromptKind,
    keymap::{Action, Scope},
};
use anyhow::Result;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
// Extended modifiers must never fall through as ordinary text or dialog answers.
const EXTENDED_MODIFIERS: KeyModifiers = KeyModifiers::SUPER
    .union(KeyModifiers::HYPER)
    .union(KeyModifiers::META);

impl App {
    pub fn event(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.key(key),
            Event::Mouse(mouse) => self.mouse(mouse),
            Event::Paste(text) => self.paste(&text),
            Event::Resize(_, _) if self.viewer.is_none() => self.reveal = true,
            _ => {}
        }
    }
    pub fn key(&mut self, key: KeyEvent) {
        if let Err(e) = self.handle_key(key) {
            self.say(format!("{e:#}"));
        }
    }
    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        self.tree_click = None;
        if self.viewer.is_some() {
            self.viewer_key(key);
            return Ok(());
        }
        if self.dialog.is_some() {
            if key.modifiers.intersects(EXTENDED_MODIFIERS)
                && !matches!(self.dialog, Some(Dialog::Settings(_)))
            {
                return Ok(());
            }
            return self.dialog_key(key);
        }
        if self.prompt.is_some() {
            return self.prompt_key(key);
        }
        let scope = if self.focus == Focus::Tree {
            Scope::Tree
        } else {
            Scope::Editor
        };
        if let Some(action) = self.keymap.action(&key, scope) {
            return self.perform(action);
        }
        if key.modifiers.intersects(EXTENDED_MODIFIERS) {
            return Ok(());
        }
        if self.focus == Focus::Tree {
            return self.tree_key(key);
        }
        self.editor_key(key)
    }
    fn tree_key(&mut self, key: KeyEvent) -> Result<()> {
        self.tree_follow = true;
        match key.code {
            KeyCode::Up => {
                self.tree_selected = self.tree_selected.saturating_sub(1);
            }
            KeyCode::Down => {
                self.tree_selected =
                    (self.tree_selected + 1).min(self.entries.len().saturating_sub(1));
            }
            KeyCode::PageUp => {
                let page = self.tree_rect.height.max(1) as usize;
                self.tree_selected = self.tree_selected.saturating_sub(page);
            }
            KeyCode::PageDown => {
                let page = self.tree_rect.height.max(1) as usize;
                self.tree_selected =
                    (self.tree_selected + page).min(self.entries.len().saturating_sub(1));
            }
            KeyCode::Home => {
                self.tree_selected = 0;
            }
            KeyCode::End => {
                self.tree_selected = self.entries.len().saturating_sub(1);
            }
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Esc => {
                self.focus = Focus::Editor;
            }
            KeyCode::Enter | KeyCode::Right => {
                self.activate_tree()?;
            }
            KeyCode::Left => {
                if let Some(e) = self.entries.get(self.tree_selected) {
                    if !self.expanded.remove(&e.path)
                        && let Some(parent) = e.path.parent()
                        && let Some(i) = self.entries.iter().position(|e| e.path == parent)
                    {
                        self.tree_selected = i;
                    }
                    self.refresh_tree()?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn activate_tree(&mut self) -> Result<()> {
        if let Some(e) = self.entries.get(self.tree_selected).cloned() {
            if e.directory {
                if !self.expanded.remove(&e.path) {
                    self.expanded.insert(e.path);
                }
                self.refresh_tree()?;
            } else {
                self.open(e.path)?;
            }
        }
        Ok(())
    }
    fn editor_key(&mut self, key: KeyEvent) -> Result<()> {
        self.build_layout();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let Some(n) = &mut self.note else {
            return Ok(());
        };
        let before = n.doc.revision;
        let dirty = n.dirty();
        let vertical = matches!(
            key.code,
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown
        );
        match key.code {
            KeyCode::Left => n.doc.horizontal(false, shift, ctrl || alt),
            KeyCode::Right => n.doc.horizontal(true, shift, ctrl || alt),
            // Option+←/→ arrive as Alt+b / Alt+f in many macOS terminals.
            KeyCode::Char('b') if alt && !ctrl => n.doc.horizontal(false, shift, true),
            KeyCode::Char('f') if alt && !ctrl => n.doc.horizontal(true, shift, true),
            KeyCode::Home => {
                let to = if ctrl { 0 } else { n.doc.line_start() };
                n.doc.set_cursor(to, shift);
            }
            KeyCode::End => {
                let to = if ctrl {
                    n.doc.text.len()
                } else {
                    n.doc.line_end()
                };
                n.doc.set_cursor(to, shift);
            }
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                if let Some(l) = &self.layout {
                    let (row, col) = l.position(n.doc.cursor);
                    let col = *self.desired_column.get_or_insert(col);
                    let distance = if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
                        self.editor_rect.height.saturating_sub(2).max(1) as usize
                    } else {
                        1
                    };
                    let y = if matches!(key.code, KeyCode::Up | KeyCode::PageUp) {
                        row.saturating_sub(distance)
                    } else {
                        (row + distance).min(l.rows.len() - 1)
                    };
                    n.doc.set_cursor(l.vertical_hit(row, y, col), shift);
                }
            }
            KeyCode::Backspace if alt || ctrl => n.doc.delete_word_back(),
            KeyCode::Char('h') if ctrl => n.doc.delete_word_back(),
            KeyCode::Backspace => n.doc.backspace(),
            KeyCode::Delete => n.doc.delete(),
            KeyCode::Enter => n.doc.newline(),
            KeyCode::Tab => {
                if !n.doc.indent(false) {
                    n.doc.insert("    ");
                }
            }
            KeyCode::BackTab => {
                n.doc.indent(true);
            }
            KeyCode::Esc => {
                if n.doc.selection().is_some() {
                    n.doc.anchor = None;
                } else {
                    self.focus = Focus::Tree;
                    self.sidebar = true;
                    self.tree_follow = true;
                }
            }
            KeyCode::Char(c) if !ctrl && !alt => n.doc.insert(&c.to_string()),
            _ => {}
        }
        if n.doc.revision != before {
            n.edited(dirty);
        }
        self.reveal = true;
        if !vertical {
            self.desired_column = None;
        }
        Ok(())
    }
    pub fn paste(&mut self, text: &str) {
        if self.dialog.is_some() || self.viewer.is_some() {
            return;
        }
        if let Some(p) = &mut self.prompt {
            let text = text.replace("\r\n", "\n").replace(['\r', '\n'], " ");
            let field = p.field_mut();
            field.break_group();
            field.insert(&text);
            field.break_group();
            self.prompt_changed();
            return;
        }
        if self.focus != Focus::Editor {
            return;
        }
        if let Some(n) = &mut self.note {
            let dirty = n.dirty();
            n.doc
                .insert(&text.replace("\r\n", "\n").replace('\r', "\n"));
            n.edited(dirty);
            self.invalidate();
        }
    }

    fn prompt_changed(&mut self) {
        if let Some(p) = &mut self.prompt
            && !p.editing_replace
        {
            p.selected = 0;
            if matches!(p.kind, PromptKind::Search) {
                self.request_search();
            }
            self.count_matches();
        }
    }

    fn prompt_key(&mut self, key: KeyEvent) -> Result<()> {
        // Only text-editing actions apply here. A global action must not close
        // the prompt or accidentally edit the underlying note.
        if let Some(action) = self.keymap.action(&key, Scope::Editor) {
            match action {
                Action::Copy | Action::Cut => {
                    let selected = self
                        .prompt
                        .as_ref()
                        .unwrap()
                        .field()
                        .selected_text()
                        .map(str::to_owned);
                    if let Some(text) = selected {
                        self.copy_text(text);
                        if action == Action::Cut {
                            self.paste("");
                        }
                    }
                    return Ok(());
                }
                Action::Paste => {
                    let text = self.clipboard_text();
                    if !text.is_empty() {
                        self.paste(&text);
                    }
                    return Ok(());
                }
                Action::Undo | Action::Redo | Action::SelectAll => {
                    let field = self.prompt.as_mut().unwrap().field_mut();
                    let revision = field.revision;
                    match action {
                        Action::Undo => field.undo(),
                        Action::Redo => field.redo(),
                        _ => field.select_all(),
                    }
                    if field.revision != revision {
                        self.prompt_changed();
                    }
                    return Ok(());
                }
                _ => {}
            }
        }
        if key.modifiers.intersects(EXTENDED_MODIFIERS) {
            return Ok(());
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let Some(p) = &mut self.prompt else {
            return Ok(());
        };
        let find = matches!(p.kind, PromptKind::Find);
        match key.code {
            KeyCode::Esc => {
                if matches!(p.kind, PromptKind::Search) {
                    self.search.cancel();
                }
                self.prompt = None;
                return Ok(());
            }
            KeyCode::Tab | KeyCode::BackTab if find => {
                p.field_mut().break_group();
                p.editing_replace = !p.editing_replace;
                if p.editing_replace && p.replacement.is_none() {
                    p.replacement = Some(Prompt::text_field(self.replace.clone(), false));
                }
                return Ok(());
            }
            // Tab switches search type while preserving text, selection and undo.
            KeyCode::Tab | KeyCode::BackTab
                if matches!(p.kind, PromptKind::Open | PromptKind::Search) =>
            {
                let search = matches!(p.kind, PromptKind::Open);
                let old = self.prompt.take().unwrap();
                self.start_prompt(if search {
                    PromptKind::Search
                } else {
                    PromptKind::Open
                })?;
                self.prompt.as_mut().unwrap().input = old.input;
                if search {
                    self.request_search();
                }
                return Ok(());
            }
            KeyCode::Char('r') if ctrl && !alt && find => {
                self.find = p.input.text.clone();
                self.replace = p
                    .replacement
                    .as_ref()
                    .map(|d| d.text.clone())
                    .unwrap_or_default();
                self.prompt = None;
                if self.find.is_empty() {
                    return Ok(());
                }
                let count = self.replace_all();
                self.say(format!("已替换 {count} 处；Ctrl+Z 可撤销"));
                return Ok(());
            }
            KeyCode::Enter if find && p.editing_replace => {
                self.find = p.input.text.clone();
                self.replace = p
                    .replacement
                    .as_ref()
                    .map(|d| d.text.clone())
                    .unwrap_or_default();
                self.replace_next();
                return Ok(());
            }
            KeyCode::Enter => {
                let p = self.prompt.take().unwrap();
                let result = self.submit(&p);
                if result.is_err() {
                    self.prompt = Some(p);
                }
                return result;
            }
            KeyCode::Up => p.selected = p.selected.saturating_sub(1),
            KeyCode::Down => p.selected = p.selected.saturating_add(1),
            KeyCode::PageUp => p.selected = p.selected.saturating_sub(8),
            KeyCode::PageDown => p.selected = p.selected.saturating_add(8),
            _ => {
                let field = p.field_mut();
                let revision = field.revision;
                match key.code {
                    KeyCode::Left => field.horizontal(false, shift, ctrl || alt),
                    KeyCode::Right => field.horizontal(true, shift, ctrl || alt),
                    KeyCode::Home => field.set_cursor(0, shift),
                    KeyCode::End => field.set_cursor(field.text.len(), shift),
                    KeyCode::Char('b') if alt && !ctrl => field.horizontal(false, shift, true),
                    KeyCode::Char('f') if alt && !ctrl => field.horizontal(true, shift, true),
                    KeyCode::Backspace if ctrl || alt => field.delete_word_back(),
                    KeyCode::Backspace => field.backspace(),
                    KeyCode::Delete => field.delete(),
                    KeyCode::Char('u') if ctrl && !alt => {
                        field.select_all();
                        field.insert("");
                    }
                    KeyCode::Char(c) if !ctrl && !alt => field.insert(&c.to_string()),
                    _ => {}
                }
                if field.revision != revision {
                    self.prompt_changed();
                }
            }
        }
        if let Some(p) = &self.prompt {
            let len = match p.kind {
                PromptKind::Search => p.hits.len(),
                _ => self.filtered(p).len(),
            };
            self.prompt.as_mut().unwrap().selected = p.selected.min(len.saturating_sub(1));
        }
        Ok(())
    }

    fn dialog_key(&mut self, key: KeyEvent) -> Result<()> {
        let dialog = self.dialog.take().unwrap();
        let scroll = |at: usize| match key.code {
            KeyCode::Up => at.saturating_sub(1),
            KeyCode::Down => at + 1,
            KeyCode::PageUp => at.saturating_sub(10),
            KeyCode::PageDown => at + 10,
            KeyCode::Home => 0,
            _ => at,
        };
        match dialog {
            Dialog::Settings(panel) => return self.settings_key(panel, key),
            Dialog::Help(at) => {
                if !matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::F(1)) {
                    self.dialog = Some(Dialog::Help(scroll(at)));
                }
            }
            Dialog::Recovery(draft) => match key.code {
                KeyCode::Enter => {
                    if let Some(n) = &mut self.note {
                        n.recover(draft);
                    }
                    self.invalidate();
                }
                KeyCode::Char('d') => {
                    if let Some(n) = &self.note {
                        n.clear_journal()?;
                    }
                }
                KeyCode::Char('q') => {
                    self.quit = true;
                }
                _ => self.dialog = Some(Dialog::Recovery(draft)),
            },
            Dialog::Delete(path) => match key.code {
                KeyCode::Enter | KeyCode::Char('y') => self.delete(&path)?,
                KeyCode::Esc | KeyCode::Char('n') => {}
                _ => self.dialog = Some(Dialog::Delete(path)),
            },
            Dialog::Conflict | Dialog::Diff(..) => {
                let in_diff = matches!(dialog, Dialog::Diff(..));
                let conflict = self.note.as_ref().is_some_and(|n| n.conflict);
                let result = match key.code {
                    KeyCode::Esc if in_diff => {
                        self.dialog = Some(Dialog::Conflict);
                        Ok(())
                    }
                    KeyCode::Esc => Ok(()),
                    KeyCode::Enter => self.keep_both(),
                    KeyCode::Char('v') if conflict => self.show_diff(),
                    KeyCode::Char('l') if conflict => self.use_disk_version(),
                    KeyCode::Char('o') => self.overwrite_with_local(),
                    KeyCode::Char('q') => {
                        if let Some(n) = &mut self.note {
                            n.journal()?;
                        }
                        self.write_session()?;
                        self.quit = true;
                        Ok(())
                    }
                    _ => {
                        self.dialog = Some(match dialog {
                            Dialog::Diff(lines, at) => Dialog::Diff(lines, scroll(at)),
                            other => other,
                        });
                        Ok(())
                    }
                };
                if result.is_err() {
                    self.dialog = Some(Dialog::Conflict);
                }
                result?;
            }
        }
        Ok(())
    }

    pub fn mouse(&mut self, event: MouseEvent) {
        if self.viewer.is_some() {
            self.viewer_mouse(event);
            return;
        }
        if matches!(self.dialog, Some(Dialog::Settings(_))) {
            self.settings_mouse(event);
            return;
        }
        if self.prompt.is_some() || self.dialog.is_some() {
            return;
        }
        let position = ratatui::layout::Position::new(event.column, event.row);
        if event.kind == MouseEventKind::Down(MouseButton::Left)
            && self.sidebar_button.contains(position)
        {
            if let Err(e) = self.perform(super::keymap::Action::ToggleSidebar) {
                self.say(format!("{e:#}"));
            }
            return;
        }
        if event.kind == MouseEventKind::Down(MouseButton::Left)
            && self.settings_button.contains(position)
        {
            self.open_settings();
            return;
        }
        if event.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some((_, url)) = self
                .image_buttons
                .iter()
                .find(|(rect, _)| rect.contains(position))
        {
            let url = url.clone();
            if let Err(e) = self.open_image(Some(url)) {
                self.say(format!("{e:#}"));
            }
            return;
        }
        let editor = self.editor_rect.contains(position);
        let tree = self.tree_rect.contains(position);
        if !matches!(
            event.kind,
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
        ) {
            self.tree_click = None;
        }
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if self.divider_rect.contains(position) => {
                self.resizing = Some(i32::from(event.column) - i32::from(self.divider_rect.x + 1));
                self.dragging = false;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.resizing.is_some() => {
                self.resize_sidebar(
                    i32::from(event.column) - i32::from(self.body_rect.x) - self.resizing.unwrap(),
                );
            }
            MouseEventKind::Down(MouseButton::Left) if self.tree_area.contains(position) => {
                self.focus = Focus::Tree;
                self.dragging = false;
                let previous = self.tree_click.take();
                if let Some(row) = tree
                    .then(|| (event.row - self.tree_rect.y) as usize + self.tree_scroll)
                    .filter(|row| *row < self.entries.len())
                {
                    self.tree_selected = row;
                    let entry = self.entries[row].clone();
                    let double = previous.is_some_and(|(path, at)| {
                        path == entry.path && at.elapsed() < std::time::Duration::from_millis(400)
                    });
                    if !double {
                        self.tree_click = Some((entry.path.clone(), std::time::Instant::now()));
                    }
                    if (!entry.directory || !double)
                        && let Err(e) = self.activate_tree()
                    {
                        self.say(format!("{e:#}"));
                    }
                    // Opening a note normally focuses the editor. A single click
                    // keeps file operations available in the pane the user clicked.
                    if !double || entry.directory {
                        self.focus = Focus::Tree;
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Left) if editor => {
                self.tree_click = None;
                self.focus = Focus::Editor;
                self.build_layout();
                if let (Some(n), Some(l)) = (&mut self.note, &self.layout) {
                    let at = l.hit(
                        (event.row - self.editor_rect.y) as usize + n.scroll,
                        (event.column - self.editor_rect.x) as usize,
                    );
                    let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);
                    n.doc
                        .set_cursor(at, event.modifiers.contains(KeyModifiers::SHIFT));
                    self.dragging = !ctrl;
                    self.reveal = true;
                    // Ctrl+click follows the link under the pointer.
                    if ctrl && let Err(e) = self.perform(super::keymap::Action::FollowLink) {
                        self.say(format!("{e:#}"));
                    }
                }
                self.desired_column = None;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging && editor => {
                self.build_layout();
                if let (Some(n), Some(l)) = (&mut self.note, &self.layout) {
                    let at = l.hit(
                        (event.row - self.editor_rect.y) as usize + n.scroll,
                        (event.column - self.editor_rect.x) as usize,
                    );
                    n.doc.set_cursor(at, true);
                    self.reveal = true;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.dragging = false;
                if self.resizing.take().is_some()
                    && let Err(e) = self.write_session()
                {
                    self.say(format!("栏宽记忆失败：{e:#}"));
                }
            }
            MouseEventKind::ScrollDown if editor => {
                if let (Some(n), Some(l)) = (&mut self.note, &self.layout) {
                    n.scroll = (n.scroll + 3).min(
                        l.rows
                            .len()
                            .saturating_sub(self.editor_rect.height as usize),
                    );
                }
                self.reveal = false;
            }
            MouseEventKind::ScrollUp if editor => {
                if let Some(n) = &mut self.note {
                    n.scroll = n.scroll.saturating_sub(3);
                }
                self.reveal = false;
            }
            // The wheel scrolls the tree; it does not move the selection.
            MouseEventKind::ScrollDown if tree => {
                let max = self
                    .entries
                    .len()
                    .saturating_sub(self.tree_rect.height as usize);
                self.tree_scroll = (self.tree_scroll + 3).min(max);
                self.tree_follow = false;
            }
            MouseEventKind::ScrollUp if tree => {
                self.tree_scroll = self.tree_scroll.saturating_sub(3);
                self.tree_follow = false;
            }
            _ => {}
        }
    }
}
