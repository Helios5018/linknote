//! Named actions and their key bindings. Users can override bindings in `keys.json`.
use anyhow::{Context, Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;
use std::{collections::HashMap, fs, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Help,
    Settings,
    Quit,
    Save,
    Create,
    NewNote,
    NewFolder,
    Rename,
    RenameTree,
    CopyPath,
    Move,
    MoveTree,
    Delete,
    OpenNote,
    SearchNotes,
    Find,
    FindNext,
    FollowLink,
    Back,
    SwitchFocus,
    ToggleSidebar,
    NarrowSidebar,
    WidenSidebar,
    ViewImage,
    ReloadImages,
    LoadRemoteImages,
    Resolve,
    Undo,
    Redo,
    SelectAll,
    Copy,
    Cut,
    Paste,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Scope {
    Global,
    Tree,
    Editor,
}

impl Action {
    pub const ALL: [Action; 33] = [
        Action::Help,
        Action::Settings,
        Action::Quit,
        Action::Save,
        Action::Create,
        Action::NewNote,
        Action::NewFolder,
        Action::Rename,
        Action::RenameTree,
        Action::CopyPath,
        Action::Move,
        Action::MoveTree,
        Action::Delete,
        Action::OpenNote,
        Action::SearchNotes,
        Action::Find,
        Action::FindNext,
        Action::FollowLink,
        Action::Back,
        Action::SwitchFocus,
        Action::ToggleSidebar,
        Action::NarrowSidebar,
        Action::WidenSidebar,
        Action::ViewImage,
        Action::ReloadImages,
        Action::LoadRemoteImages,
        Action::Resolve,
        Action::Undo,
        Action::Redo,
        Action::SelectAll,
        Action::Copy,
        Action::Cut,
        Action::Paste,
    ];

    /// (config name, description, scope, default keys)
    fn spec(self) -> (&'static str, &'static str, Scope, &'static [&'static str]) {
        use Scope::*;
        match self {
            Action::Help => ("help", "帮助", Global, &["f1"]),
            Action::Settings => ("settings", "设置", Global, &["ctrl+,"]),
            Action::Quit => ("quit", "保存并退出", Global, &["ctrl+q"]),
            Action::Save => ("save", "立即保存（平时自动保存）", Global, &["ctrl+s"]),
            Action::Create => (
                "create",
                "新建笔记 / 文件夹（左栏）",
                Tree,
                &["a", "shift+a"],
            ),
            Action::NewNote => ("new_note", "新建笔记 / 文件夹", Global, &[]),
            Action::NewFolder => ("new_folder", "新建文件夹", Global, &[]),
            Action::Rename => ("rename", "重命名（全局，可选）", Global, &[]),
            Action::RenameTree => ("rename_tree", "重命名（左栏）", Tree, &["r"]),
            Action::CopyPath => ("copy_path", "复制绝对路径（左栏）", Tree, &["c"]),
            Action::Move => ("move", "移动到其他文件夹（全局，可选）", Global, &[]),
            Action::MoveTree => ("move_tree", "移动到其他文件夹（左栏）", Tree, &["m"]),
            Action::Delete => (
                "delete",
                "确认后移到废纸篓（左栏）",
                Tree,
                &["d", "shift+d"],
            ),
            Action::OpenNote => ("open_note", "查找笔记（Tab 切换全文）", Global, &["ctrl+p"]),
            Action::SearchNotes => ("search_notes", "全文搜索", Global, &[]),
            Action::Find => ("find", "文内查找 / 替换", Global, &["ctrl+f"]),
            Action::FindNext => ("find_next", "下一个匹配", Global, &["ctrl+g"]),
            Action::FollowLink => ("follow_link", "打开光标处的链接", Editor, &["f12"]),
            Action::Back => ("back", "回到跳转前的笔记", Global, &["shift+f12"]),
            Action::SwitchFocus => ("switch_focus", "切换左右栏", Global, &["ctrl+w"]),
            Action::ToggleSidebar => ("toggle_sidebar", "展开/收起左栏", Global, &["ctrl+b"]),
            Action::NarrowSidebar => ("narrow_sidebar", "缩窄左栏", Global, &[]),
            Action::WidenSidebar => ("widen_sidebar", "加宽左栏", Global, &[]),
            Action::ViewImage => ("view_image", "放大图片 / Mermaid", Editor, &["f10"]),
            Action::ReloadImages => ("reload_images", "重新加载图片", Global, &[]),
            Action::LoadRemoteImages => {
                ("load_remote_images", "加载/停止本篇的网络图片", Global, &[])
            }
            Action::Resolve => ("resolve", "处理保存冲突 / 重试保存", Global, &["ctrl+r"]),
            Action::Undo if cfg!(target_os = "macos") => {
                ("undo", "撤销", Editor, &["cmd+z", "ctrl+z"])
            }
            Action::Redo if cfg!(target_os = "macos") => (
                "redo",
                "重做",
                Editor,
                &["cmd+shift+z", "ctrl+y", "ctrl+shift+z"],
            ),
            Action::SelectAll if cfg!(target_os = "macos") => {
                ("select_all", "全选", Editor, &["cmd+a", "ctrl+a"])
            }
            Action::Copy if cfg!(target_os = "macos") => {
                ("copy", "复制", Editor, &["cmd+c", "ctrl+c"])
            }
            Action::Cut if cfg!(target_os = "macos") => {
                ("cut", "剪切", Editor, &["cmd+x", "ctrl+x"])
            }
            Action::Paste if cfg!(target_os = "macos") => {
                ("paste", "粘贴", Editor, &["cmd+v", "ctrl+v"])
            }
            Action::Undo => ("undo", "撤销", Editor, &["ctrl+z"]),
            Action::Redo => ("redo", "重做", Editor, &["ctrl+y", "ctrl+shift+z"]),
            Action::SelectAll => ("select_all", "全选", Editor, &["ctrl+a"]),
            Action::Copy => ("copy", "复制", Editor, &["ctrl+c"]),
            Action::Cut => ("cut", "剪切", Editor, &["ctrl+x"]),
            Action::Paste => ("paste", "粘贴", Editor, &["ctrl+v"]),
        }
    }
    pub fn name(self) -> &'static str {
        self.spec().0
    }
    pub fn label(self) -> &'static str {
        self.spec().1
    }
    fn scope(self) -> Scope {
        self.spec().2
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    code: KeyCode,
    mods: KeyModifiers,
}

fn normalize(code: KeyCode, mods: KeyModifiers) -> Binding {
    let mut mods = mods;
    let code = match code {
        KeyCode::Char(c) if c.is_ascii_uppercase() => {
            mods |= KeyModifiers::SHIFT;
            KeyCode::Char(c.to_ascii_lowercase())
        }
        KeyCode::BackTab => {
            mods |= KeyModifiers::SHIFT;
            KeyCode::Tab
        }
        other => other,
    };
    Binding { code, mods }
}

impl Binding {
    pub fn parse(spec: &str) -> Result<Self> {
        let spec = spec.trim().to_ascii_lowercase();
        let mut parts: Vec<&str> = spec.split('+').collect();
        // "ctrl++" binds the plus key.
        if spec.ends_with("++") {
            parts.truncate(parts.len() - 2);
            parts.push("+");
        }
        let key = parts.pop().filter(|k| !k.is_empty()).context("缺少按键")?;
        let mut mods = KeyModifiers::NONE;
        for m in parts {
            mods |= match m {
                "ctrl" | "control" => KeyModifiers::CONTROL,
                "alt" | "option" | "meta" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                "cmd" | "command" | "super" => KeyModifiers::SUPER,
                other => bail!("未知修饰键 {other}"),
            };
        }
        let code = match key {
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" => KeyCode::Insert,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "space" => KeyCode::Char(' '),
            k if k.len() > 1 && k.starts_with('f') && k[1..].parse::<u8>().is_ok() => {
                let n: u8 = k[1..].parse()?;
                if !(1..=24).contains(&n) {
                    bail!("功能键超出范围：{k}");
                }
                KeyCode::F(n)
            }
            k if k.chars().count() == 1 => KeyCode::Char(k.chars().next().unwrap()),
            other => bail!("未知按键 {other}"),
        };
        Ok(normalize(code, mods))
    }
    pub fn matches(&self, key: &KeyEvent) -> bool {
        normalize(key.code, key.modifiers) == *self
    }
    pub fn display(&self) -> String {
        let mut out = String::new();
        for (m, name) in [
            (KeyModifiers::SUPER, "Cmd+"),
            (KeyModifiers::CONTROL, "Ctrl+"),
            (KeyModifiers::ALT, "Alt+"),
            (KeyModifiers::SHIFT, "Shift+"),
        ] {
            if self.mods.contains(m) {
                out.push_str(name);
            }
        }
        out.push_str(&match self.code {
            KeyCode::Char(' ') => "Space".into(),
            KeyCode::Char(c) => c.to_ascii_uppercase().to_string(),
            KeyCode::F(n) => format!("F{n}"),
            KeyCode::PageUp => "PageUp".into(),
            KeyCode::PageDown => "PageDown".into(),
            other => format!("{other:?}"),
        });
        out
    }
}

#[derive(Deserialize)]
struct Config {
    #[serde(default)]
    keys: HashMap<String, Vec<String>>,
}

pub struct Keymap {
    bindings: Vec<(Binding, Action)>,
    /// Problems in the user's configuration, shown once at startup.
    pub warning: Option<String>,
    source: Option<Vec<u8>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::build(&HashMap::new())
    }
}

impl Keymap {
    /// Read `keys.json`; a missing file means defaults. Invalid entries are reported
    /// and the defaults for those actions are kept.
    pub fn load(path: &Path) -> Self {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(_) => return Self::default(),
        };
        let config: Config = match serde_json::from_str(&text) {
            Ok(config) => config,
            Err(e) => {
                return Self {
                    warning: Some(format!("快捷键配置无法解析，已使用默认值：{e}")),
                    source: Some(text.as_bytes().to_vec()),
                    ..Self::default()
                };
            }
        };
        let mut problems = vec![];
        let mut chosen = HashMap::new();
        for (name, keys) in &config.keys {
            let Some(action) = Action::ALL.iter().find(|a| a.name() == name) else {
                problems.push(format!("未知动作 {name}"));
                continue;
            };
            let parsed: Result<Vec<_>> = keys.iter().map(|k| Binding::parse(k)).collect();
            match parsed {
                Ok(bindings) => {
                    chosen.insert(*action, bindings);
                }
                Err(e) => problems.push(format!("{name}: {e:#}")),
            }
        }
        let mut map = Self::build(&chosen);
        map.source = Some(text.into_bytes());
        if !problems.is_empty() {
            map.warning = Some(format!("快捷键配置有误（{}）", problems.join("；")));
        }
        map
    }
    fn build(chosen: &HashMap<Action, Vec<Binding>>) -> Self {
        let mut bindings = vec![];
        // Configured bindings come first, so they win over a default of another action.
        for action in Action::ALL {
            if let Some(keys) = chosen.get(&action) {
                bindings.extend(keys.iter().map(|b| (*b, action)));
            }
        }
        for action in Action::ALL {
            if !chosen.contains_key(&action) {
                for key in action.spec().3 {
                    bindings.push((Binding::parse(key).expect("valid default"), action));
                }
            }
        }
        Self {
            bindings,
            warning: None,
            source: None,
        }
    }
    pub fn action(&self, key: &KeyEvent, scope: Scope) -> Option<Action> {
        self.bindings
            .iter()
            .find(|(b, a)| b.matches(key) && (a.scope() == Scope::Global || a.scope() == scope))
            .map(|(_, a)| *a)
    }
    pub fn keys(&self, action: Action) -> String {
        let keys: Vec<_> = self
            .bindings
            .iter()
            .filter(|(_, a)| *a == action)
            .map(|(b, _)| b.display())
            .collect();
        if keys.is_empty() {
            "未绑定".into()
        } else {
            keys.join(" / ")
        }
    }

    /// Replace one action's bindings (or restore its defaults), preserving the
    /// rest of keys.json. Refuse conflicts rather than silently stealing keys.
    pub fn assign(&mut self, path: &Path, action: Action, key: Option<KeyEvent>) -> Result<()> {
        let specs: Vec<String> = if let Some(key) = key {
            if !matches!(key.code, KeyCode::F(_))
                && !(action.scope() == Scope::Tree
                    && matches!(key.code, KeyCode::Char(c) if !c.is_control()))
                && !(matches!(key.code, KeyCode::Char(_))
                    && key.modifiers.intersects(
                        KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                    ))
            {
                bail!("请使用功能键或 Ctrl/Alt/Cmd 组合键；左栏动作也可使用单字母");
            }
            vec![normalize(key.code, key.modifiers).display()]
        } else {
            action.spec().3.iter().map(|s| s.to_string()).collect()
        };
        for spec in &specs {
            let binding = Binding::parse(spec)?;
            if let Some((_, other)) = self.bindings.iter().find(|(b, a)| {
                *b == binding
                    && *a != action
                    && (a.scope() == Scope::Global
                        || action.scope() == Scope::Global
                        || a.scope() == action.scope())
            }) {
                bail!("{} 已用于「{}」", binding.display(), other.label());
            }
        }
        let bytes = match fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        if bytes != self.source {
            bail!("快捷键已被其他窗口修改，请关闭后重开设置");
        }
        let mut value: serde_json::Value = bytes
            .as_ref()
            .map(|b| serde_json::from_slice(b))
            .transpose()?
            .unwrap_or_else(|| serde_json::json!({}));
        let root = value.as_object_mut().context("keys.json 必须是对象")?;
        let keys = root
            .entry("keys")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .context("keys 必须是对象")?;
        if key.is_some() {
            keys.insert(action.name().into(), serde_json::json!(specs));
        } else {
            keys.remove(action.name());
        }
        fs::create_dir_all(path.parent().context("快捷键目录不存在")?)?;
        crate::storage::atomic_write(path, &serde_json::to_vec_pretty(&value)?)?;
        *self = Self::load(path);
        Ok(())
    }
}
