//! Following links from a note: Markdown links, `[[wiki links]]`, headings and URLs.
use crate::tree;
use anyhow::{Context, Result};
use percent_encoding::percent_decode_str;
use std::{
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Debug, PartialEq)]
pub enum Target {
    /// An existing note, optionally with a heading anchor.
    Note(PathBuf, Option<String>),
    /// A note path that does not exist yet.
    Missing(PathBuf),
    /// A heading in the current note.
    Heading(String),
    /// A folder inside the notes root.
    Folder(PathBuf),
    /// A URL or a non-Markdown file, handed to the system opener.
    External(String),
    /// A path outside the notes root.
    Outside,
}

/// Resolve `.` and `..` without touching the file system.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

fn with_markdown_extension(path: &Path) -> PathBuf {
    if tree::markdown(path) {
        path.to_path_buf()
    } else {
        let mut name = path.as_os_str().to_owned();
        name.push(".md");
        PathBuf::from(name)
    }
}

pub fn resolve(root: &Path, note: &Path, dest: &str, wiki: bool) -> Target {
    let dest = dest.trim();
    if let Some(heading) = dest.strip_prefix('#') {
        return Target::Heading(heading.to_owned());
    }
    if dest.contains("://") || dest.starts_with("mailto:") {
        return Target::External(dest.to_owned());
    }
    let (path, heading) = match dest.split_once('#') {
        Some((p, h)) => (p, Some(h.to_owned()).filter(|h| !h.is_empty())),
        None => (dest, None),
    };
    let path = if wiki {
        path.to_owned()
    } else {
        percent_decode_str(path).decode_utf8_lossy().into_owned()
    };
    let dir = note.parent().unwrap_or(root);
    if wiki {
        let wanted = with_markdown_extension(Path::new(&path));
        for candidate in [dir.join(&wanted), root.join(&wanted)] {
            let candidate = normalize(&candidate);
            if candidate.starts_with(root) && candidate.is_file() {
                return Target::Note(candidate, heading);
            }
        }
        // Like other wiki-style tools, `[[name]]` finds the note anywhere in the tree;
        // the shortest matching path wins.
        let name = wanted.to_string_lossy().to_lowercase();
        let found = tree::all_notes(root)
            .unwrap_or_default()
            .into_iter()
            .filter(|p| {
                let relative = p
                    .strip_prefix(root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .to_lowercase();
                relative == name || relative.ends_with(&format!("/{name}"))
            })
            .min_by_key(|p| p.components().count());
        return match found {
            Some(p) => Target::Note(p, heading),
            None => Target::Missing(normalize(&dir.join(wanted))),
        };
    }
    // `/name.md` is relative to the notes root unless it is already a path inside it.
    let target = match path.strip_prefix('/') {
        Some(_) if Path::new(&path).starts_with(root) => PathBuf::from(&path),
        Some(rest) => root.join(rest),
        None => dir.join(&path),
    };
    let target = normalize(&target);
    if !target.starts_with(root) {
        return Target::Outside;
    }
    if target.is_dir() {
        return Target::Folder(target);
    }
    if target.is_file() {
        return if tree::markdown(&target) {
            Target::Note(target, heading)
        } else {
            Target::External(target.to_string_lossy().into_owned())
        };
    }
    if target.extension().is_none() || tree::markdown(&target) {
        Target::Missing(with_markdown_extension(&target))
    } else {
        Target::External(target.to_string_lossy().into_owned())
    }
}

/// A bare `http(s)://` URL in plain text around `offset`.
pub fn url_at(text: &str, offset: usize) -> Option<String> {
    let start = text[..offset]
        .rfind(char::is_whitespace)
        .map_or(0, |i| i + text[i..].chars().next().unwrap().len_utf8());
    let end = text[offset..]
        .find(char::is_whitespace)
        .map_or(text.len(), |i| offset + i);
    let word = &text[start..end];
    let at = word.find("https://").or_else(|| word.find("http://"))?;
    let url = word[at..].trim_end_matches(|c: char| {
        matches!(
            c,
            ')' | ']' | '>' | '.' | ',' | ';' | '!' | '?' | '"' | '\''
        ) || !c.is_ascii()
    });
    (url.len() > "https://".len()).then(|| url.to_owned())
}

/// GitHub-style heading anchor.
pub fn slug(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| {
            if c.is_whitespace() {
                Some('-')
            } else if c.is_alphanumeric() || c == '-' || c == '_' {
                Some(c)
            } else {
                None
            }
        })
        .collect()
}

/// Open a URL or file with the system handler (or `LINKNOTE_OPENER`).
pub fn open_external(target: &str, opener: Option<&str>) -> Result<()> {
    let mut command = match opener {
        Some(program) => Command::new(program),
        None if cfg!(target_os = "macos") => Command::new("open"),
        None if cfg!(windows) => {
            let mut c = Command::new("cmd");
            c.args(["/C", "start", ""]);
            c
        }
        None => Command::new("xdg-open"),
    };
    command
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("无法调用系统程序打开链接")?;
    Ok(())
}
