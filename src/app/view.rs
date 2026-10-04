use super::{App, Dialog, Focus, PromptKind, keymap::Action};
use crate::{document::Document, search};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use similar::ChangeTag;
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// A single-line view over the same editing core used by the note. Scroll by
/// grapheme boundaries and reserve one terminal cell for the insertion cursor.
fn input_line(
    doc: &Document,
    label: String,
    width: usize,
    palette: &super::theme::Palette,
) -> (Line<'static>, usize) {
    let available = width.saturating_sub(1);
    let mut start = doc.cursor;
    let mut column = 0;
    for (offset, grapheme) in doc.text[..doc.cursor].grapheme_indices(true).rev() {
        let cells = UnicodeWidthStr::width(grapheme);
        if column + cells > available {
            break;
        }
        column += cells;
        start = offset;
    }
    let cursor = UnicodeWidthStr::width(label.as_str()) + column;
    let mut spans = vec![Span::raw(label)];
    let selected = doc.selection();
    let mut used = 0;
    for (offset, grapheme) in doc.text[start..].grapheme_indices(true) {
        let cells = UnicodeWidthStr::width(grapheme);
        if used + cells > width {
            break;
        }
        let style = if selected
            .as_ref()
            .is_some_and(|r| r.contains(&(start + offset)))
        {
            Style::default().fg(palette.fg).bg(palette.select)
        } else {
            Style::default()
        };
        spans.push(Span::styled(grapheme.to_owned(), style));
        used += cells;
    }
    (Line::from(spans), cursor)
}

impl App {
    pub fn draw(&mut self, f: &mut Frame) {
        if let Some(viewer) = &mut self.viewer {
            viewer.draw(f, self.settings.theme);
            return;
        }
        self.image_buttons.clear();
        let palette = self.settings.theme.palette();
        let area = f.area();
        self.body_rect = Rect::default();
        self.settings_button = Rect::default();
        self.sidebar_button = Rect::default();
        self.divider_rect = Rect::default();
        self.tree_rect = Rect::default();
        self.tree_area = Rect::default();
        self.editor_rect = Rect::default();
        f.render_widget(
            Block::default().style(Style::default().bg(palette.bg).fg(palette.fg)),
            area,
        );
        if area.width < 34 || area.height < 9 {
            f.render_widget(
                Paragraph::new(format!(
                    "终端太小，请放大至 34 × 9 以上。\n{} 仍可保存并退出。",
                    self.keymap.keys(Action::Quit)
                )),
                area,
            );
            return;
        }
        let top = Rect::new(area.x, area.y, area.width, 2);
        let root_name = self.root.file_name().unwrap_or_default().to_string_lossy();
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("  LinkNote  ", Style::default().fg(palette.accent).bold()),
                Span::styled(format!("/ {root_name}"), Style::default().fg(palette.dim)),
            ])),
            top,
        );
        let settings_label = format!(
            " 设置 {} ",
            self.keymap
                .keys(Action::Settings)
                .split(" / ")
                .next()
                .unwrap_or("")
        );
        let settings_width =
            (UnicodeWidthStr::width(settings_label.as_str()) as u16).min(area.width / 2);
        self.settings_button =
            Rect::new(area.right() - settings_width - 1, area.y, settings_width, 1);
        f.render_widget(
            Paragraph::new(settings_label)
                .style(Style::default().fg(palette.accent).bg(palette.select)),
            self.settings_button,
        );
        let body = Rect::new(area.x, area.y + 2, area.width, area.height - 3);
        self.body_rect = body;
        let tree_width = if self.sidebar_visible() {
            if area.width >= 64 {
                self.sidebar_columns(area.width)
            } else {
                area.width
            }
        } else {
            0
        };
        let tree_area = Rect::new(body.x, body.y, tree_width, body.height);
        let editor_area = Rect::new(
            body.x + tree_width,
            body.y,
            body.width - tree_width,
            body.height,
        );
        if tree_width > 0 && editor_area.width > 0 {
            self.divider_rect = Rect::new(body.x + tree_width - 1, body.y, 2, body.height);
        }
        if tree_width > 0 {
            self.draw_tree(f, tree_area);
        }
        if editor_area.width > 0 {
            let title = self
                .note
                .as_ref()
                .map(|n| {
                    n.path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                })
                .unwrap_or_else(|| "开始记录".into());
            let title = if tree_width == 0 {
                format!("  {title} ")
            } else {
                format!(" {title} ")
            };
            let border = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if self.focus == Focus::Editor {
                    palette.accent
                } else {
                    palette.dim
                }))
                .title(title);
            let inner = border.inner(editor_area);
            f.render_widget(border, editor_area);
            self.editor_rect = Rect::new(
                inner.x + 1,
                inner.y + 1,
                inner.width.saturating_sub(2),
                inner.height.saturating_sub(2),
            );
            self.build_layout();
            self.draw_editor(f);
        }
        let sidebar_label = if tree_width > 0 { "<" } else { ">" };
        self.sidebar_button = Rect::new(
            if tree_width > 0 {
                tree_area.right() - 2
            } else {
                body.x + 1
            },
            body.y,
            1,
            1,
        );
        f.render_widget(
            Paragraph::new(sidebar_label).style(Style::default().fg(palette.accent)),
            self.sidebar_button,
        );
        self.draw_status(
            f,
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
        if self.prompt.is_some() {
            self.draw_prompt(f);
        }
        if self.dialog.is_some() {
            self.draw_dialog(f);
        }
    }
    /// Save state and position, followed by the latest message for a few seconds.
    fn draw_status(&self, f: &mut Frame, area: Rect) {
        let palette = self.settings.theme.palette();
        let mut spans = vec![];
        if let Some(n) = &self.note {
            let (line, col) = n.doc.line_column();
            let (saved, color) = if n.conflict {
                (
                    format!("磁盘冲突 · {} 处理", self.keymap.keys(Action::Resolve)),
                    palette.warn,
                )
            } else if let Some(e) = &n.error {
                (e.clone(), palette.warn)
            } else if let Some(e) = &n.journal_error {
                (format!("恢复草稿写入失败：{e}"), palette.warn)
            } else if n.dirty() {
                ("保存中…".into(), palette.accent)
            } else {
                ("已保存".into(), palette.accent)
            };
            spans.push(Span::styled(
                format!("  {saved}"),
                Style::default().fg(color),
            ));
            spans.push(Span::styled(
                format!(
                    "    {line}:{col}    {}",
                    if n.crlf { "CRLF" } else { "UTF-8" }
                ),
                Style::default().fg(palette.accent),
            ));
        } else {
            spans.push(Span::styled(
                format!(
                    "  选择笔记，或在左栏 {} 新建 · {} 帮助",
                    self.keymap.keys(Action::Create),
                    self.keymap.keys(Action::Help)
                ),
                Style::default().fg(palette.dim),
            ));
        }
        if let Some((message, at)) = &self.message
            && at.elapsed() < Duration::from_secs(8)
        {
            spans.push(Span::styled("    │ ", Style::default().fg(palette.dim)));
            spans.push(Span::styled(
                message.clone(),
                Style::default().fg(palette.fg),
            ));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }
    fn draw_tree(&mut self, f: &mut Frame, area: Rect) {
        self.tree_area = area;
        let palette = self.settings.theme.palette();
        let border = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if self.focus == Focus::Tree {
                palette.accent
            } else {
                Color::Rgb(54, 65, 80)
            }))
            .title(" 笔记 ");
        self.tree_rect = border.inner(area);
        f.render_widget(border, area);
        let height = self.tree_rect.height as usize;
        if self.tree_follow {
            if self.tree_selected < self.tree_scroll {
                self.tree_scroll = self.tree_selected;
            }
            if self.tree_selected >= self.tree_scroll + height {
                self.tree_scroll = self.tree_selected + 1 - height;
            }
            self.tree_follow = false;
        }
        self.tree_scroll = self
            .tree_scroll
            .min(self.entries.len().saturating_sub(height));
        let lines: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .skip(self.tree_scroll)
            .take(height)
            .map(|(i, e)| {
                let icon = if e.directory {
                    if self.expanded.contains(&e.path) {
                        "▾ "
                    } else {
                        "▸ "
                    }
                } else {
                    "  "
                };
                let name = e.path.file_name().unwrap_or_default().to_string_lossy();
                let current = self.note.as_ref().is_some_and(|n| n.path == e.path);
                let mut style = Style::default().fg(if e.directory {
                    palette.dim
                } else if current {
                    palette.accent
                } else {
                    palette.fg
                });
                if i == self.tree_selected {
                    style = style.bg(palette.select);
                }
                let mut spans = vec![Span::styled(
                    format!(" {}{icon}{name}", "  ".repeat(e.depth)),
                    style,
                )];
                if self.drafts.contains(&e.path) {
                    spans.push(Span::styled(" ●", style.fg(palette.warn)));
                }
                Line::from(spans)
            })
            .collect();
        f.render_widget(Paragraph::new(lines), self.tree_rect);
        if self.entries.is_empty() {
            f.render_widget(
                Paragraph::new(format!(
                    "\n 空文件夹\n\n {} 新建\n 名称 → .md\n 名称/ → 文件夹",
                    self.keymap.keys(Action::Create)
                ))
                .style(Style::default().fg(palette.dim)),
                self.tree_rect,
            );
        }
    }
    fn draw_editor(&mut self, f: &mut Frame) {
        let palette = self.settings.theme.palette();
        let rect = self.editor_rect;
        if rect.width < 2 || rect.height == 0 {
            return;
        }
        let (Some(n), Some(l)) = (&mut self.note, &self.layout) else {
            f.render_widget(
                Paragraph::new(format!(
                    "把想法留在这里。\n\n左栏 {} 新建 Markdown 笔记\n{} 查找笔记 · Tab 切换全文搜索\n\n打开后直接输入，修改自动保存。",
                    self.keymap.keys(Action::Create),
                    self.keymap.keys(Action::OpenNote),
                ))
                .style(Style::default().fg(palette.dim)),
                rect,
            );
            return;
        };
        let (cy, cx) = l.position(n.doc.cursor);
        let h = rect.height as usize;
        if self.reveal {
            if cy < n.scroll {
                n.scroll = cy;
            }
            if cy >= n.scroll + h {
                n.scroll = cy + 1 - h;
            }
        }
        n.scroll = n.scroll.min(l.rows.len().saturating_sub(1));
        self.reveal = false;
        let selection = n.doc.selection();
        let lines: Vec<_> = l
            .rows
            .iter()
            .skip(n.scroll)
            .take(h)
            .map(|row| {
                let spans: Vec<_> = row
                    .glyphs
                    .iter()
                    .map(|g| {
                        let selected = selection
                            .as_ref()
                            .is_some_and(|s| g.source.start < s.end && g.source.end > s.start);
                        Span::styled(
                            g.text.as_str(),
                            if selected {
                                self.settings.theme.text_style(g.style).bg(palette.select)
                            } else {
                                self.settings.theme.text_style(g.style)
                            },
                        )
                    })
                    .collect();
                Line::from(spans)
            })
            .collect();
        f.render_widget(Paragraph::new(lines), rect);
        if self.prompt.is_none() && self.dialog.is_none() {
            for image in &l.images {
                let canvas = Rect {
                    width: rect.width.saturating_sub(4),
                    ..rect
                };
                self.images.draw(f, canvas, image, n.scroll);
                // Reserve a rail outside the native graphics area: terminal image
                // protocols can paint over text placed on top of their pixels.
                let row = image.row.max(n.scroll);
                if row < image.row + image.height && row < n.scroll + h && rect.width >= 4 {
                    let button =
                        Rect::new(rect.right() - 4, rect.y + (row - n.scroll) as u16, 4, 1);
                    f.render_widget(
                        Paragraph::new(" 🔍 ")
                            .style(Style::default().fg(palette.accent).bg(palette.select)),
                        button,
                    );
                    self.image_buttons.push((button, image.url.clone()));
                }
            }
        }
        if self.focus == Focus::Editor
            && self.prompt.is_none()
            && self.dialog.is_none()
            && cy >= n.scroll
            && cy < n.scroll + h
        {
            f.set_cursor_position((
                rect.x + (cx as u16).min(rect.width - 1),
                rect.y + (cy - n.scroll) as u16,
            ));
        }
    }
    fn popup(area: Rect, width: u16, height: u16) -> Rect {
        let w = width.min(area.width.saturating_sub(4));
        let h = height.min(area.height.saturating_sub(2));
        Rect::new(
            area.x + (area.width - w) / 2,
            area.y + (area.height - h) / 2,
            w,
            h,
        )
    }
    fn draw_prompt(&self, f: &mut Frame) {
        let palette = self.settings.theme.palette();
        let p = self.prompt.as_ref().unwrap();
        let creation = match &p.kind {
            PromptKind::Create(parent) => Some(("新建", parent, ".md 笔记 · / 文件夹")),
            PromptKind::NewNote(parent) => Some(("新建笔记", parent, "默认 .md")),
            PromptKind::NewFolder(parent) => Some(("新建文件夹", parent, "输入文件夹名称")),
            _ => None,
        };
        if let Some((title, parent, hint)) = creation {
            let rect = Self::popup(f.area(), 56, 4);
            let block = Block::default()
                .borders(Borders::ALL)
                .title(format!(" {title} · {} ", self.relative(parent)))
                .title_bottom(" Enter 确认 · Esc 取消 ")
                .style(Style::default().bg(palette.bg).fg(palette.accent));
            let inner = block.inner(rect);
            let (line, column) = input_line(
                &p.input,
                "> ".into(),
                inner.width.saturating_sub(4) as usize,
                &palette,
            );
            f.render_widget(Clear, rect);
            f.render_widget(block, rect);
            f.render_widget(
                Paragraph::new(vec![
                    line,
                    Line::styled(hint, Style::default().fg(palette.dim)),
                ]),
                inner,
            );
            f.set_cursor_position((inner.x + column as u16, inner.y));
            return;
        }
        let switch = "Tab 切换文件名 / 全文";
        let (title, context) = match &p.kind {
            PromptKind::Create(parent) => (
                "新建笔记 / 文件夹",
                format!("位置：{}", self.relative(parent)),
            ),
            PromptKind::NewNote(parent) => ("新建笔记", format!("位置：{}", self.relative(parent))),
            PromptKind::NewFolder(parent) => {
                ("新建文件夹", format!("位置：{}", self.relative(parent)))
            }
            PromptKind::Rename(_) => ("重命名", "Ctrl+U 清空原名".into()),
            PromptKind::Move(path) => (
                "移动到文件夹",
                format!("移动「{}」 · 输入筛选 · ↑↓ 选择", self.relative(path)),
            ),
            PromptKind::Find => {
                let count = p.count;
                let case = if search::insensitive(&p.input.text) {
                    "不区分大小写"
                } else {
                    "区分大小写"
                };
                (
                    "文内查找",
                    format!("{count} 处匹配 · {case}（输入大写字母时区分）"),
                )
            }
            PromptKind::Open => ("打开笔记", format!("输入路径片段 · ↑↓ 选择 · {switch}")),
            PromptKind::Search => {
                let state = if p.searching {
                    "搜索中…".to_owned()
                } else if p.input.text.trim().is_empty() {
                    "输入要搜索的内容".to_owned()
                } else if p.truncated {
                    format!("前 {} 处匹配", p.hits.len())
                } else {
                    format!("{} 处匹配", p.hits.len())
                };
                ("全文搜索", format!("{state} · {switch}"))
            }
        };
        let list = matches!(
            p.kind,
            PromptKind::Open | PromptKind::Search | PromptKind::Move(_)
        );
        let rect = Self::popup(
            f.area(),
            if list { 88 } else { 72 },
            if list {
                f.area().height.saturating_sub(4).clamp(9, 22)
            } else {
                8
            },
        );
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {title} "))
            .style(Style::default().bg(palette.bg).fg(palette.accent));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let width = inner.width.saturating_sub(10) as usize;
        let find = matches!(p.kind, PromptKind::Find);
        let label = |name: &str| {
            if find {
                format!("{name} > ")
            } else {
                "> ".into()
            }
        };
        let (line, column) = input_line(&p.input, label("查找"), width, &palette);
        let mut lines = vec![
            Line::styled(context, Style::default().fg(palette.dim)),
            line,
        ];
        let mut cursor = (inner.x + column as u16, inner.y + 1);
        if let Some(replace) = &p.replacement {
            let (line, column) = input_line(replace, label("替换"), width, &palette);
            lines.push(line);
            if p.editing_replace {
                cursor = (inner.x + column as u16, inner.y + 2);
            }
        }
        lines.push(Line::raw(""));
        let rows: Vec<(String, String)> = match p.kind {
            PromptKind::Search => p
                .hits
                .iter()
                .map(|h| {
                    (
                        format!("{}:{}", self.relative(&h.path), h.line),
                        h.snippet.clone(),
                    )
                })
                .collect(),
            _ if list => self
                .filtered(p)
                .iter()
                .map(|path| (self.relative(path), String::new()))
                .collect(),
            _ => vec![],
        };
        let visible = inner.height.saturating_sub(lines.len() as u16) as usize;
        let selected = p.selected.min(rows.len().saturating_sub(1));
        let skip = (selected + 1).saturating_sub(visible);
        for (i, (head, detail)) in rows.iter().enumerate().skip(skip).take(visible) {
            let bg = if i == selected {
                palette.select
            } else {
                palette.bg
            };
            lines.push(Line::from(vec![
                Span::styled(format!(" {head}"), Style::default().fg(palette.fg).bg(bg)),
                Span::styled(
                    if detail.is_empty() {
                        String::new()
                    } else {
                        format!("  {detail}")
                    },
                    Style::default().fg(palette.dim).bg(bg),
                ),
            ]));
        }
        if list && rows.is_empty() && !p.searching && !p.input.text.trim().is_empty() {
            lines.push(Line::styled(" 没有匹配", Style::default().fg(palette.dim)));
        }
        if find {
            lines.push(Line::styled(
                "Enter 查找下一个 · Tab 切换查找 / 替换栏",
                Style::default().fg(palette.dim),
            ));
            lines.push(Line::styled(
                "替换栏中 Enter 逐个替换 · Ctrl+R 全部替换 · Esc 关闭",
                Style::default().fg(palette.dim),
            ));
        } else if !list {
            lines.push(Line::styled(
                "Enter 确定 · Esc 取消",
                Style::default().fg(palette.dim),
            ));
        }
        f.render_widget(Paragraph::new(lines), inner);
        f.set_cursor_position(cursor);
    }
    fn help_text(&self) -> String {
        let mut out = String::from("直接输入即可编辑；光标所在块显示 Markdown 源码。\n\n");
        for action in super::keymap::Action::ALL {
            let keys = self.keymap.keys(action);
            if keys == "未绑定" {
                continue;
            }
            let pad = 22usize
                .saturating_sub(UnicodeWidthStr::width(keys.as_str()))
                .max(2);
            out.push_str(&format!("{keys}{}{}\n", " ".repeat(pad), action.label()));
        }
        out.push_str(
            "\n左栏\n\
             单击文件 / 空白 / 标题  聚焦左栏；单击 Markdown 同时在右侧打开\n\
             Enter / 双击笔记       进入右侧编辑器\n\
             ↑↓ / PageUp / PageDown 选择文件；← 折叠或到父目录；→ 打开\n\
             Tab / Esc             返回编辑器\n\
             新建                  名称补 .md；拒绝其他格式；末尾 / 创建文件夹\n\n\
             编辑\n\
             Shift+方向键          选择文字（正文和输入框均支持）\n\
             Ctrl/Alt+←/→          按词移动（中文按标点和文字类型分段）\n\
             Alt+Backspace         删除前一个词\n\
             Home / End            行首 / 行尾；加 Ctrl 到文首 / 文末\n\
             Tab / Shift+Tab       列表项缩进 / 取消缩进；其他行插入四个空格\n\
             Enter                 延续列表；空的子项回到上一级，空的顶层项结束列表\n\
             Esc                   清除选区；没有选区时回到左栏\n\
             鼠标                  点击定位、拖动选区、滚轮滚动；Ctrl+点击打开链接\n\n\
             停止输入 500ms 自动保存，持续输入最长每 2s 保存。\n\
             快捷键可在应用配置目录的 keys.json 中修改，详见 README。\n\n\
             ↑↓ 滚动 · Esc / Enter 关闭",
        );
        out
    }
    fn draw_dialog(&self, f: &mut Frame) {
        let palette = self.settings.theme.palette();
        let (title, lines, scroll, height): (&str, Vec<Line>, usize, u16) = match self
            .dialog
            .as_ref()
            .unwrap()
        {
            Dialog::Settings(panel) => {
                self.draw_settings(f, panel);
                return;
            }
            Dialog::Help(at) => (
                "快捷键",
                self.help_text()
                    .lines()
                    .map(|l| Line::raw(l.to_owned()))
                    .collect(),
                *at,
                40,
            ),
            Dialog::Conflict => {
                let conflict = self.note.as_ref().is_some_and(|n| n.conflict);
                let mut text = String::from(if conflict {
                    "磁盘上的文件在你编辑期间被修改或删除，自动保存已暂停。\n\n\
                         v      查看两个版本的差异\n\
                         Enter  把本地内容另存为新的 .local-N.md 并打开（两份都保留）\n\
                         l      改用磁盘版本（本地内容可用 Ctrl+Z 找回）\n\
                         o      用本地版本覆盖磁盘文件\n"
                } else {
                    "保存失败，内容仍在编辑器和恢复草稿中。\n\n\
                         Enter  把本地内容另存为新的 .local-N.md 并打开\n\
                         o      重试保存到原文件\n"
                });
                text.push_str(
                    "Esc    回到编辑器，继续保留当前内容\n\
                         q      保留恢复草稿并退出，下次启动可恢复\n\n\
                         底部状态栏显示具体原因。",
                );
                (
                    "保护未保存的内容",
                    text.lines().map(|l| Line::raw(l.to_owned())).collect(),
                    0,
                    13,
                )
            }
            Dialog::Diff(diff, at) => {
                let mut lines = vec![
                    Line::styled(
                        "- 磁盘版本    + 本地版本 · ↑↓ 滚动 · Enter 两份都保留 · l 用磁盘 · o 用本地 · Esc 返回",
                        Style::default().fg(palette.dim),
                    ),
                    Line::raw(""),
                ];
                lines.extend(diff.iter().map(|d| match d.tag {
                    ChangeTag::Delete => Line::styled(
                        format!("- {}", d.text),
                        Style::default().fg(palette.removed),
                    ),
                    ChangeTag::Insert => {
                        Line::styled(format!("+ {}", d.text), Style::default().fg(palette.added))
                    }
                    ChangeTag::Equal => {
                        Line::styled(format!("  {}", d.text), Style::default().fg(palette.dim))
                    }
                }));
                ("磁盘版本与本地版本的差异", lines, *at, 40)
            }
            Dialog::Recovery(_) => (
                "发现未完成的草稿",
                "上次退出前有修改尚未写回 Markdown。\n\n\
                     Enter  恢复草稿（磁盘也有改动时会进入冲突处理）\n\
                     d      丢弃恢复草稿，使用当前磁盘内容\n\
                     q      保留草稿并退出\n\n\
                     请选择后继续，原文件目前没有被修改。"
                    .lines()
                    .map(|l| Line::raw(l.to_owned()))
                    .collect(),
                0,
                11,
            ),
            Dialog::Delete(path) => (
                "移到废纸篓",
                format!(
                    "把「{}」移到废纸篓？\n\n{}\n\nEnter 确认 · Esc 取消",
                    self.relative(path),
                    if path.is_dir() {
                        "文件夹及其中所有内容都会被移走。"
                    } else {
                        "之后可以从废纸篓恢复。"
                    }
                )
                .lines()
                .map(|l| Line::raw(l.to_owned()))
                .collect(),
                0,
                9,
            ),
        };
        let rect = Self::popup(f.area(), 92, height);
        let visible = rect.height.saturating_sub(2) as usize;
        let scroll = scroll.min(lines.len().saturating_sub(visible));
        f.render_widget(Clear, rect);
        f.render_widget(
            Paragraph::new(lines)
                .scroll((scroll as u16, 0))
                .style(Style::default().bg(palette.bg).fg(palette.fg))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" {title} "))
                        .border_style(Style::default().fg(palette.accent)),
                ),
            rect,
        );
    }
}
