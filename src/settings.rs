//! User preferences, kept outside notes and written atomically.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, io::ErrorKind, path::PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MermaidTheme {
    #[default]
    Auto,
    Dark,
    Default,
    Forest,
    Neutral,
    Base,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Canvas {
    #[default]
    Auto,
    Dark,
    Light,
    Transparent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageDisplay {
    #[default]
    Auto,
    Characters,
    Off,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: Theme,
    pub sidebar_visible: bool,
    pub mermaid_theme: MermaidTheme,
    pub mermaid_canvas: Canvas,
    pub mermaid_font_size: u16,
    pub image_display: ImageDisplay,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            sidebar_visible: true,
            mermaid_theme: MermaidTheme::Auto,
            mermaid_canvas: Canvas::Auto,
            mermaid_font_size: 16,
            image_display: ImageDisplay::Auto,
            extra: BTreeMap::new(),
        }
    }
}

impl Settings {
    fn validate(&self) -> Result<()> {
        if !(12..=32).contains(&self.mermaid_font_size) {
            bail!("Mermaid 字号应在 12–32 之间");
        }
        Ok(())
    }

    pub fn value(&self, row: usize) -> String {
        match row {
            0 => match self.mermaid_theme {
                MermaidTheme::Auto => "跟随整体主题",
                MermaidTheme::Dark => "深色",
                MermaidTheme::Default => "默认彩色",
                MermaidTheme::Forest => "森林绿",
                MermaidTheme::Neutral => "黑白",
                MermaidTheme::Base => "自定义基础",
            }
            .into(),
            1 => match self.mermaid_canvas {
                Canvas::Auto => "跟随图表主题",
                Canvas::Dark => "深色",
                Canvas::Light => "浅色",
                Canvas::Transparent => "透明",
            }
            .into(),
            2 => format!("{} px", self.mermaid_font_size),
            _ => match self.image_display {
                ImageDisplay::Auto => "自动",
                ImageDisplay::Characters => "字符图",
                ImageDisplay::Off => "关闭",
            }
            .into(),
        }
    }

    pub fn change(&mut self, row: usize, forward: bool) {
        fn cycle<T: Copy + PartialEq>(value: &mut T, values: &[T], forward: bool) {
            let i = values.iter().position(|v| v == value).unwrap_or(0);
            *value = values[(i + if forward { 1 } else { values.len() - 1 }) % values.len()];
        }
        match row {
            0 => cycle(
                &mut self.mermaid_theme,
                &[
                    MermaidTheme::Auto,
                    MermaidTheme::Dark,
                    MermaidTheme::Default,
                    MermaidTheme::Forest,
                    MermaidTheme::Neutral,
                    MermaidTheme::Base,
                ],
                forward,
            ),
            1 => cycle(
                &mut self.mermaid_canvas,
                &[
                    Canvas::Auto,
                    Canvas::Dark,
                    Canvas::Light,
                    Canvas::Transparent,
                ],
                forward,
            ),
            2 => {
                self.mermaid_font_size = if forward {
                    (self.mermaid_font_size + 2).min(32)
                } else {
                    self.mermaid_font_size.saturating_sub(2).max(12)
                }
            }
            _ => cycle(
                &mut self.image_display,
                &[
                    ImageDisplay::Auto,
                    ImageDisplay::Characters,
                    ImageDisplay::Off,
                ],
                forward,
            ),
        }
    }
}

pub struct Store {
    path: PathBuf,
    snapshot: Option<Vec<u8>>,
    pub warning: Option<String>,
}

impl Store {
    pub fn load(path: PathBuf) -> (Self, Settings) {
        let mut store = Self {
            path,
            snapshot: None,
            warning: None,
        };
        let result = (|| -> Result<Settings> {
            store.snapshot = match fs::read(&store.path) {
                Ok(bytes) => Some(bytes),
                Err(e) if e.kind() == ErrorKind::NotFound => None,
                Err(e) => return Err(e.into()),
            };
            let settings: Settings = store
                .snapshot
                .as_ref()
                .map(|bytes| serde_json::from_slice(bytes))
                .transpose()?
                .unwrap_or_default();
            settings.validate()?;
            Ok(settings)
        })();
        let settings = match result {
            Ok(settings) => settings,
            Err(e) => {
                store.warning = Some(format!("设置文件无法读取，暂用默认值：{e}"));
                Settings::default()
            }
        };
        (store, settings)
    }

    pub fn save(&mut self, settings: &Settings) -> Result<()> {
        if self.warning.is_some() {
            bail!("请修复 settings.json 后重开设置，原配置未覆盖");
        }
        settings.validate()?;
        let current = match fs::read(&self.path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        if current != self.snapshot {
            bail!("设置已被其他窗口修改，请关闭后重开设置再调整");
        }
        fs::create_dir_all(self.path.parent().context("设置目录不存在")?)?;
        let bytes = serde_json::to_vec_pretty(settings)?;
        crate::storage::atomic_write(&self.path, &bytes).context("设置保存失败")?;
        self.snapshot = Some(bytes);
        Ok(())
    }

    pub fn reload(&self) -> (Self, Settings) {
        Self::load(self.path.clone())
    }
}
