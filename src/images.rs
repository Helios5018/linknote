use anyhow::{Context, Result, bail};
use image::{DynamicImage, ImageReader, Limits};
use percent_encoding::percent_decode_str;
use ratatui::{
    Frame,
    layout::Rect,
    layout::Size,
    style::{Color, Style},
    widgets::{Paragraph, Wrap},
};
use ratatui_image::{
    picker::{Picker, ProtocolType, cap_parser::QueryStdioOptions},
    sliced::{SlicedImage, SlicedProtocol},
};
use std::{
    collections::HashMap,
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::Duration,
};

const MAX_BYTES: u64 = 12 * 1024 * 1024;

pub fn picker() -> Picker {
    let override_protocol = std::env::var("LINKNOTE_IMAGE_PROTOCOL").unwrap_or_default();
    let mut picker = if override_protocol == "halfblocks" || override_protocol == "off" {
        Picker::halfblocks()
    } else {
        let timeout = Duration::from_millis(500);
        let started = std::time::Instant::now();
        let picker = Picker::from_query_stdio_with_options(QueryStdioOptions {
            timeout,
            ..Default::default()
        })
        .unwrap_or_else(|_| Picker::halfblocks());
        // The library reports an unanswered query as a fallback picker, so the only
        // sign of it is the time spent waiting.
        if started.elapsed() >= timeout {
            release_query_reader();
        }
        picker
    };
    match override_protocol.as_str() {
        "kitty" => picker.set_protocol_type(ProtocolType::Kitty),
        "sixel" => picker.set_protocol_type(ProtocolType::Sixel),
        "iterm2" => picker.set_protocol_type(ProtocolType::Iterm2),
        _ => {}
    }
    picker.set_background_color(Some(image::Rgba([22, 27, 34, 255])));
    picker
}

/// After an unanswered capability query (for example tmux without passthrough),
/// ratatui-image's reader thread stays blocked on stdin and would swallow the user's
/// first key or paste. Request a status report that tmux and terminals answer
/// themselves, so that thread reads the reply and exits before input is handled.
fn release_query_reader() {
    use std::io::Write;
    let mut out = std::io::stdout();
    if out.write_all(b"\x1b[5n").and_then(|_| out.flush()).is_ok() {
        std::thread::sleep(Duration::from_millis(150));
    }
}

pub fn load_image(note: &Path, url: &str) -> Result<DynamicImage> {
    if let Some(source) = crate::mermaid::source(url) {
        return crate::mermaid::render(source);
    }
    let image = load_full_image(note, url)?;
    Ok(if image.width() > 1200 || image.height() > 1200 {
        image.thumbnail(1200, 1200)
    } else {
        image
    })
}

/// Decode on demand for the viewer, retaining the original pixels within the same limits.
pub fn load_full_image(note: &Path, url: &str) -> Result<DynamicImage> {
    let bytes = if is_remote(url) {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(8)))
            .max_redirects(3)
            .build()
            .into();
        agent
            .get(url)
            .call()
            .context("图片下载失败")?
            .body_mut()
            .with_config()
            .limit(MAX_BYTES)
            .read_to_vec()?
    } else {
        if url.contains("://") || url.starts_with("data:") {
            bail!("支持本地路径和 HTTP(S) 图片");
        }
        let path = percent_decode_str(url)
            .decode_utf8()
            .context("图片路径不是 UTF-8")?;
        let path = note.parent().unwrap_or(Path::new(".")).join(path.as_ref());
        let meta = fs::metadata(&path).with_context(|| format!("找不到图片 {}", path.display()))?;
        if !meta.is_file() || meta.len() > MAX_BYTES {
            bail!("图片必须是 12 MiB 以内的普通文件");
        }
        let mut bytes = vec![];
        fs::File::open(path)?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)?;
        bytes
    };
    if bytes.len() as u64 > MAX_BYTES {
        bail!("图片超过 12 MiB");
    }
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    reader
        .decode()
        .context("无法解码图片（支持 PNG/JPEG/WebP/GIF 首帧）")
}

pub fn is_remote(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

enum State {
    /// Remote images wait for explicit permission; loading them reveals the reader's IP.
    Blocked,
    Pending,
    Loading,
    Ready(SlicedProtocol),
    Failed(String),
}
struct Entry {
    original: Option<Arc<DynamicImage>>,
    state: State,
}
struct Finished {
    generation: u64,
    url: String,
    result: Result<(Arc<DynamicImage>, SlicedProtocol), String>,
}

pub struct Images {
    picker: Picker,
    auto_picker: Picker,
    settings: crate::settings::Settings,
    enabled: bool,
    remote: bool,
    always_remote: bool,
    entries: HashMap<String, Entry>,
    key: Option<(PathBuf, u64, u16)>,
    generation: u64,
    in_flight: usize,
    tx: mpsc::Sender<Finished>,
    rx: mpsc::Receiver<Finished>,
}

impl Images {
    pub fn new(picker: Picker) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            auto_picker: picker.clone(),
            picker,
            settings: crate::settings::Settings::default(),
            enabled: std::env::var("LINKNOTE_IMAGE_PROTOCOL").as_deref() != Ok("off"),
            remote: false,
            always_remote: std::env::var("LINKNOTE_REMOTE_IMAGES").as_deref() == Ok("1"),
            entries: HashMap::new(),
            key: None,
            generation: 0,
            in_flight: 0,
            tx,
            rx,
        }
    }
    pub fn viewer_picker(&self, url: &str) -> Result<Picker> {
        if !self.enabled {
            bail!("图片显示已关闭，请在设置中开启");
        }
        if is_remote(url) && !self.remote && !self.always_remote {
            bail!("网络图片未加载，请先在设置中允许本篇网络图片");
        }
        Ok(self.picker.clone())
    }
    pub fn configure_picker(&mut self, picker: Picker) {
        self.auto_picker = picker;
        self.reconfigure();
    }
    pub fn configure(&mut self, settings: &crate::settings::Settings) {
        let changed = self.settings.theme != settings.theme
            || self.settings.mermaid_theme != settings.mermaid_theme
            || self.settings.mermaid_canvas != settings.mermaid_canvas
            || self.settings.mermaid_font_size != settings.mermaid_font_size
            || self.settings.image_display != settings.image_display;
        self.settings = settings.clone();
        if changed {
            self.reconfigure();
        }
    }
    fn reconfigure(&mut self) {
        use crate::settings::{ImageDisplay, Theme};
        self.picker = if self.settings.image_display == ImageDisplay::Characters {
            Picker::halfblocks()
        } else {
            self.auto_picker.clone()
        };
        self.enabled = match self.settings.image_display {
            ImageDisplay::Off => false,
            ImageDisplay::Characters => true,
            ImageDisplay::Auto => std::env::var("LINKNOTE_IMAGE_PROTOCOL").as_deref() != Ok("off"),
        };
        self.picker
            .set_background_color(Some(match self.settings.theme {
                Theme::Dark => image::Rgba([22, 27, 34, 255]),
                Theme::Light => image::Rgba([247, 248, 250, 255]),
            }));
        self.generation += 1;
        // Retain the in-flight counter; old jobs still occupy a worker until
        // completion, but their previous-theme results can no longer be displayed.
        self.entries.clear();
        self.key = None;
    }
    fn initial(&self, url: &str) -> State {
        if is_remote(url) && !self.remote && !self.always_remote {
            State::Blocked
        } else {
            State::Pending
        }
    }
    pub fn refresh(&mut self) {
        self.generation += 1;
        let states: Vec<_> = self
            .entries
            .keys()
            .map(|u| (u.clone(), self.initial(u)))
            .collect();
        for (url, state) in states {
            let entry = self.entries.get_mut(&url).unwrap();
            entry.original = None;
            entry.state = state;
        }
        self.launch();
    }
    /// Allow or block network images for the current note.
    pub fn set_remote(&mut self, allowed: bool) {
        if self.remote == allowed {
            return;
        }
        self.remote = allowed;
        for (url, entry) in &mut self.entries {
            match entry.state {
                State::Blocked if allowed => entry.state = State::Pending,
                _ if !allowed && is_remote(url) && !self.always_remote => {
                    entry.state = State::Blocked;
                    entry.original = None;
                }
                _ => {}
            }
            // Generation changes invalidate local jobs too. Requeue them so an
            // Permission changes during Mermaid rendering cannot leave a loading slot forever.
            if matches!(entry.state, State::Loading) {
                entry.state = State::Pending;
            }
        }
        self.generation += 1;
        self.launch();
    }
    pub fn has_blocked(&self) -> bool {
        self.entries
            .values()
            .any(|e| matches!(e.state, State::Blocked))
    }
    pub fn sync(&mut self, note: &Path, revision: u64, text: &str, width: u16) {
        let urls = crate::layout::image_sources(text);
        self.sync_urls(note, revision, &urls, width);
    }
    /// Returns true when the set of tracked images changed.
    pub fn sync_urls(&mut self, note: &Path, revision: u64, urls: &[String], width: u16) -> bool {
        if !self.enabled || width < 2 {
            return false;
        }
        let key = (note.to_path_buf(), revision, width);
        if self.key.as_ref() == Some(&key) {
            return false;
        }
        let before: Vec<String> = self.entries.keys().cloned().collect();
        if self.key.as_ref().is_none_or(|(p, _, _)| p != note) {
            self.entries.clear();
            self.generation += 1;
        } else if self.key.as_ref().is_some_and(|(_, _, w)| *w != width) {
            self.generation += 1;
            for entry in self.entries.values_mut() {
                if !matches!(entry.state, State::Blocked) {
                    entry.state = State::Pending;
                }
            }
        }
        self.entries.retain(|url, _| urls.contains(url));
        for url in urls {
            if self.entries.len() >= 16 {
                break;
            }
            if !self.entries.contains_key(url) {
                let state = self.initial(url);
                self.entries.insert(
                    url.clone(),
                    Entry {
                        original: None,
                        state,
                    },
                );
            }
        }
        self.key = Some(key);
        self.launch();
        before.len() != self.entries.len() || before.iter().any(|u| !self.entries.contains_key(u))
    }
    fn launch(&mut self) {
        if !self.enabled {
            return;
        }
        let Some((path, _, width)) = &self.key else {
            return;
        };
        for (url, entry) in &mut self.entries {
            if self.in_flight >= 2 {
                break;
            }
            if !matches!(entry.state, State::Pending) {
                continue;
            }
            let original = entry.original.clone();
            let picker = self.picker.clone();
            let path = path.clone();
            let url = url.clone();
            let width = *width;
            let tx = self.tx.clone();
            let generation = self.generation;
            let settings = self.settings.clone();
            entry.state = State::Loading;
            self.in_flight += 1;
            std::thread::spawn(move || {
                let result = (|| -> Result<_> {
                    let original = match original {
                        Some(image) => image,
                        None if crate::mermaid::source(&url).is_some() => {
                            Arc::new(crate::mermaid::render_with_settings(
                                crate::mermaid::source(&url).unwrap(),
                                &settings,
                            )?)
                        }
                        None => Arc::new(load_image(&path, &url)?),
                    };
                    let image = SlicedProtocol::new(
                        &picker,
                        (*original).clone(),
                        Some(if crate::mermaid::source(&url).is_some() {
                            Size::new(width, crate::mermaid::MAX_ROWS as u16)
                        } else {
                            Size::new(width.min(96), 18)
                        }),
                    )?;
                    Ok((original, image))
                })()
                .map_err(|e| format!("{e:#}"));
                let _ = tx.send(Finished {
                    generation,
                    url,
                    result,
                });
            });
        }
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(done) = self.rx.try_recv() {
            self.in_flight = self.in_flight.saturating_sub(1);
            if done.generation == self.generation
                && let Some(entry) = self.entries.get_mut(&done.url)
            {
                match done.result {
                    Ok((original, protocol)) => {
                        entry.original = Some(original);
                        entry.state = State::Ready(protocol);
                    }
                    Err(error) => entry.state = State::Failed(error),
                }
                changed = true;
            }
        }
        self.launch();
        changed
    }
    pub fn heights(&self) -> HashMap<String, usize> {
        self.entries
            .iter()
            .map(|(url, entry)| {
                (
                    url.clone(),
                    match &entry.state {
                        State::Ready(p) => usize::from(p.size().height).max(1),
                        State::Failed(_) if crate::mermaid::source(url).is_some() => 3,
                        _ => 1,
                    },
                )
            })
            .collect()
    }
    pub fn draw(&self, f: &mut Frame, rect: Rect, slot: &crate::layout::ImageSlot, scroll: usize) {
        let Some(entry) = self.entries.get(&slot.url) else {
            return;
        };
        let y = slot.row as i64 - scroll as i64;
        if y + slot.height as i64 <= 0 || y >= i64::from(rect.height) {
            return;
        }
        match &entry.state {
            State::Ready(protocol) => {
                f.render_widget(SlicedImage::new(protocol, (0, y as i16).into()), rect)
            }
            _ if y >= 0 => {
                let mermaid = crate::mermaid::source(&slot.url).is_some();
                let text = match &entry.state {
                    State::Failed(e) if mermaid => format!("Mermaid 无法渲染：{e}"),
                    State::Failed(e) => format!("图片无法加载：{e}"),
                    State::Blocked => "网络图片未加载 · 在设置中允许本篇网络图片".into(),
                    _ if mermaid => "Mermaid 渲染中…".into(),
                    _ => "图片加载中…".into(),
                };
                f.render_widget(
                    Paragraph::new(text)
                        .wrap(Wrap { trim: false })
                        .style(Style::default().fg(Color::DarkGray)),
                    Rect::new(
                        rect.x,
                        rect.y + y as u16,
                        rect.width,
                        (slot.height as u16).min(rect.height - y as u16),
                    ),
                );
            }
            _ => {}
        }
    }
}
