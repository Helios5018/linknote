use super::{
    App, Dialog,
    keymap::{Action, Scope},
};
use crate::settings::{Settings, Theme};
use anyhow::{Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Margin, Position, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const CATEGORIES: [&str; 3] = ["外观", "图表与图片", "快捷键"];

#[derive(Default)]
pub(super) struct Panel {
    category: usize,
    selected: usize,
    capture: Option<Action>,
    error: Option<String>,
}

impl Panel {
    fn count(&self) -> usize {
        match self.category {
            0 => 3,
            1 => 6,
            _ => Action::ALL.len(),
        }
    }
    fn category(&mut self, next: usize) {
        self.category = next;
        self.selected = 0;
        self.capture = None;
        self.error = None;
    }
}

fn geometry(area: Rect, selected: usize, count: usize) -> (Rect, Rect, usize) {
    let height = (count as u16 + 9).min(24);
    let rect = Rect::new(
        area.x + 2,
        area.y + 2,
        area.width.saturating_sub(4),
        area.height.saturating_sub(4),
    );
    let rect = Rect::new(
        rect.x + rect.width.saturating_sub(88) / 2,
        rect.y + rect.height.saturating_sub(height) / 2,
        rect.width.min(88),
        rect.height.min(height),
    );
    let inner = rect.inner(Margin::new(1, 1));
    let reserve = if inner.height >= 10 { 5 } else { 1 };
    let list = Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height.saturating_sub(1 + reserve).max(1),
    );
    let scroll = selected.saturating_sub(list.height as usize - 1);
    (rect, list, scroll)
}

fn clipped(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.into();
    }
    let mut out = String::new();
    let mut used = 0;
    for g in text.graphemes(true) {
        let w = UnicodeWidthStr::width(g);
        if used + w + 1 > width {
            break;
        }
        out.push_str(g);
        used += w;
    }
    if width > 0 {
        out.push('…');
    }
    out
}

impl App {
    pub(super) fn open_settings(&mut self) {
        let (store, settings) = self.settings_store.reload();
        let mut error = store.warning.clone();
        if error.is_none() {
            self.apply_settings(settings);
        }
        self.settings_store = store;
        self.keymap = super::keymap::Keymap::load(&self.keys_path);
        if error.is_none() {
            error = self.keymap.warning.clone();
        }
        self.resizing = None;
        self.dragging = false;
        self.dialog = Some(Dialog::Settings(Panel {
            error,
            ..Default::default()
        }));
    }

    fn apply_settings(&mut self, settings: Settings) {
        if settings.sidebar_visible != self.settings.sidebar_visible {
            self.sidebar = settings.sidebar_visible;
        }
        self.images.configure(&settings);
        self.settings = settings;
        self.layout_key = None;
        self.reveal = true;
    }

    fn change_setting(&mut self, panel: &Panel, forward: bool) -> Result<()> {
        match (panel.category, panel.selected) {
            (0, 2) => {
                let previous = self.session.sidebar_width;
                self.resize_sidebar(
                    i32::from(self.sidebar_columns(self.body_rect.width))
                        + if forward { 2 } else { -2 },
                );
                if let Err(error) = self.write_session() {
                    self.session.sidebar_width = previous;
                    return Err(error);
                }
            }
            (1, 5) => self.perform(Action::ReloadImages)?,
            (1, 4) => {
                if self.note.is_none() {
                    bail!("打开笔记后可设置本篇网络图片");
                }
                if std::env::var("LINKNOTE_REMOTE_IMAGES").as_deref() == Ok("1") {
                    bail!("网络图片由 LINKNOTE_REMOTE_IMAGES=1 开启，请移除该环境配置后重启");
                }
                self.perform(Action::LoadRemoteImages)?;
            }
            _ => {
                let mut next = self.settings.clone();
                if panel.category == 0 {
                    if panel.selected == 0 {
                        next.theme = if next.theme == Theme::Dark {
                            Theme::Light
                        } else {
                            Theme::Dark
                        };
                    } else {
                        next.sidebar_visible = !self.sidebar;
                    }
                } else {
                    next.change(panel.selected, forward);
                }
                self.settings_store.save(&next)?;
                let sidebar = next.sidebar_visible;
                self.apply_settings(next);
                if panel.category == 0 && panel.selected == 1 {
                    self.sidebar = sidebar;
                }
            }
        }
        Ok(())
    }

    pub(super) fn settings_key(&mut self, mut panel: Panel, key: KeyEvent) -> Result<()> {
        if let Some(action) = panel.capture {
            if key.code == KeyCode::Esc {
                panel.capture = None;
                panel.error = None;
            } else {
                match self.keymap.assign(&self.keys_path, action, Some(key)) {
                    Ok(()) => {
                        panel.capture = None;
                        panel.error = None;
                    }
                    Err(e) => panel.error = Some(format!("{e:#}")),
                }
            }
            if let Some(error) = &panel.error {
                self.say(error.clone());
            }
            self.dialog = Some(Dialog::Settings(panel));
            return Ok(());
        }
        if key.code == KeyCode::Esc
            || self.keymap.action(&key, Scope::Global) == Some(Action::Settings)
        {
            return Ok(());
        }
        match key.code {
            KeyCode::Tab => panel.category((panel.category + 1) % 3),
            KeyCode::BackTab => panel.category((panel.category + 2) % 3),
            KeyCode::Up => panel.selected = panel.selected.saturating_sub(1),
            KeyCode::Down => panel.selected = (panel.selected + 1).min(panel.count() - 1),
            KeyCode::Home => panel.selected = 0,
            KeyCode::End => panel.selected = panel.count() - 1,
            KeyCode::PageUp => panel.selected = panel.selected.saturating_sub(8),
            KeyCode::PageDown => panel.selected = (panel.selected + 8).min(panel.count() - 1),
            KeyCode::Enter | KeyCode::Char(' ') if panel.category == 2 => {
                panel.capture = Some(Action::ALL[panel.selected]);
                panel.error = None;
            }
            KeyCode::Backspace if panel.category == 2 => {
                panel.error = self
                    .keymap
                    .assign(&self.keys_path, Action::ALL[panel.selected], None)
                    .err()
                    .map(|e| format!("{e:#}"));
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ')
                if panel.category != 2 =>
            {
                panel.error = self
                    .change_setting(&panel, key.code != KeyCode::Left)
                    .err()
                    .map(|e| format!("{e:#}"));
            }
            _ => {}
        }
        if let Some(error) = &panel.error {
            self.say(error.clone());
        }
        self.dialog = Some(Dialog::Settings(panel));
        Ok(())
    }

    pub(super) fn settings_mouse(&mut self, event: MouseEvent) {
        let Some(Dialog::Settings(mut panel)) = self.dialog.take() else {
            return;
        };
        let area = Rect::new(
            self.body_rect.x,
            self.body_rect.y.saturating_sub(2),
            self.body_rect.width,
            self.body_rect.height + 3,
        );
        let (rect, list, scroll) = geometry(area, panel.selected, panel.count());
        let position = Position::new(event.column, event.row);
        let mut key = None;
        if panel.capture.is_none() {
            if event.kind == MouseEventKind::Down(MouseButton::Left) {
                if event.row == rect.bottom().saturating_sub(2) && rect.contains(position) {
                    key = Some(KeyCode::Esc);
                } else if event.row == rect.y + 1 {
                    let mut x = rect.x + 1;
                    for (i, title) in CATEGORIES.iter().enumerate() {
                        let width = UnicodeWidthStr::width(*title) as u16 + 2;
                        if event.column >= x && event.column < x + width {
                            panel.category(i);
                            break;
                        }
                        x += width;
                    }
                } else if list.contains(position) {
                    let row = scroll + usize::from(event.row - list.y);
                    if row < panel.count() {
                        panel.selected = row;
                        key = Some(KeyCode::Enter);
                    }
                }
            } else if rect.contains(position) {
                key = match event.kind {
                    MouseEventKind::ScrollDown => Some(KeyCode::Down),
                    MouseEventKind::ScrollUp => Some(KeyCode::Up),
                    _ => None,
                };
            }
        }
        if let Some(key) = key {
            let _ = self.settings_key(panel, KeyEvent::new(key, KeyModifiers::NONE));
        } else {
            self.dialog = Some(Dialog::Settings(panel));
        }
    }

    fn setting_row(&self, category: usize, row: usize) -> (String, String) {
        match category {
            0 => match row {
                0 => (
                    "整体主题".into(),
                    if self.settings.theme == Theme::Dark {
                        "深色"
                    } else {
                        "浅色"
                    }
                    .into(),
                ),
                1 => (
                    "显示侧栏".into(),
                    if self.sidebar { "开启" } else { "关闭" }.into(),
                ),
                _ => (
                    "侧栏宽度".into(),
                    format!("{} 列", self.sidebar_columns(self.body_rect.width)),
                ),
            },
            1 if row < 4 => (
                ["Mermaid 主题", "画布背景", "图中文字大小", "图片显示方式"][row].into(),
                self.settings.value(row),
            ),
            1 if row == 5 => ("重新加载图片".into(), "Enter 重试 / 更新".into()),
            1 => (
                "本篇网络图片".into(),
                if self.note.is_none() {
                    "未打开笔记"
                } else if std::env::var("LINKNOTE_REMOTE_IMAGES").as_deref() == Ok("1") {
                    "环境配置允许"
                } else if self
                    .note
                    .as_ref()
                    .is_some_and(|n| self.session.remote_images.contains(&n.path))
                {
                    "允许加载"
                } else {
                    "不加载"
                }
                .into(),
            ),
            _ => (
                Action::ALL[row].label().into(),
                self.keymap.keys(Action::ALL[row]),
            ),
        }
    }

    pub(super) fn draw_settings(&self, f: &mut Frame, panel: &Panel) {
        let palette = self.settings.theme.palette();
        let (rect, list, scroll) = geometry(f.area(), panel.selected, panel.count());
        f.render_widget(Clear, rect);
        f.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .title(" 设置 ")
                .style(Style::default().bg(palette.bg).fg(palette.fg))
                .border_style(Style::default().fg(palette.accent)),
            rect,
        );
        let inner = rect.inner(Margin::new(1, 1));
        let tabs: Vec<_> = CATEGORIES
            .iter()
            .enumerate()
            .map(|(i, name)| {
                Span::styled(
                    format!(" {name} "),
                    if i == panel.category {
                        Style::default()
                            .fg(palette.accent)
                            .bg(palette.select)
                            .bold()
                    } else {
                        Style::default().fg(palette.dim)
                    },
                )
            })
            .collect();
        f.render_widget(
            Paragraph::new(Line::from(tabs)),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        for (offset, row) in (scroll..panel.count())
            .take(list.height as usize)
            .enumerate()
        {
            let (label, value) = self.setting_row(panel.category, row);
            let label_width = (list.width as usize / 2).saturating_sub(2);
            let label = clipped(&label, label_width);
            let value = clipped(
                &value,
                (list.width as usize).saturating_sub(label_width + 4),
            );
            let text = format!(
                "{} {}{}{value}",
                if row == panel.selected { "›" } else { " " },
                label,
                " ".repeat(label_width.saturating_sub(UnicodeWidthStr::width(label.as_str())) + 2)
            );
            let style = if row == panel.selected {
                Style::default().fg(palette.accent).bg(palette.select)
            } else {
                Style::default().fg(palette.fg)
            };
            f.render_widget(
                Paragraph::new(text).style(style),
                Rect::new(list.x, list.y + offset as u16, list.width, 1),
            );
        }
        if inner.height >= 10 {
            let description = if let Some(action) = panel.capture {
                format!("按下「{}」的新快捷键；Esc 取消录入。", action.label())
            } else if let Some(error) = &panel.error {
                error.clone()
            } else if panel.category == 2 {
                format!(
                    "{}：{}\nEnter 录入 · Backspace 恢复默认；冲突会提示。",
                    Action::ALL[panel.selected].label(),
                    self.keymap.keys(Action::ALL[panel.selected])
                )
            } else {
                match (panel.category, panel.selected) {
                    (0, 0) => "影响正文、侧栏和弹窗；图表默认跟随整体主题。",
                    (0, 1) => "立即切换，并记住下次启动时是否显示侧栏。",
                    (0, _) => "仅记住当前笔记目录的栏宽，窄终端会临时收窄。",
                    (1, 0) => "这是默认主题；单张图源码中的配置优先。",
                    (1, 1) => "跟随主题会自动匹配深浅背景；透明会融入编辑器。",
                    (1, 2) => "调整图中文字；大图仍会按可用宽度缩放。",
                    (1, 3) => "自动使用终端能力；字符图清晰度有限；关闭时可编辑源码。",
                    (1, 5) => "重试加载失败的图片，并更新本地图片与 Mermaid 预览。",
                    _ => "仅对此笔记生效；允许后会访问图片地址。",
                }
                .into()
            };
            let description = if let (Some(_), Some(error)) = (panel.capture, &panel.error) {
                format!("{error}\n{description}")
            } else {
                description
            };
            f.render_widget(
                Paragraph::new(description)
                    .wrap(Wrap { trim: false })
                    .style(Style::default().fg(if panel.error.is_some() {
                        palette.warn
                    } else {
                        palette.dim
                    })),
                Rect::new(inner.x, list.bottom() + 1, inner.width, 3),
            );
        }
        let footer = if inner.width < 50 {
            "Tab 分类 · ←→ 调整 · Esc 关闭"
        } else {
            "Tab 分类 · ↑↓ 选择 · ←→ 调整 · 自动保存 · Esc 关闭"
        };
        f.render_widget(
            Paragraph::new(footer).style(Style::default().fg(palette.dim)),
            Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1),
        );
    }
}
