use anyhow::{Result, bail};
use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub depth: usize,
    pub directory: bool,
}
pub fn markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("md") || s.eq_ignore_ascii_case("markdown"))
}

/// Names hidden from the tree and searches: entries starting with `.`, `node_modules`,
/// and patterns from `<root>/.linknoteignore` (one name per line, `*` and `?`
/// wildcards, a trailing `/` matches folders only, `#` starts a comment).
pub struct Ignore {
    patterns: Vec<(String, bool)>,
}

pub const IGNORE_FILE: &str = ".linknoteignore";

fn wildcard(pattern: &[char], name: &[char]) -> bool {
    match (pattern.first(), name.first()) {
        (None, None) => true,
        (Some('*'), _) => {
            wildcard(&pattern[1..], name) || (!name.is_empty() && wildcard(pattern, &name[1..]))
        }
        (Some('?'), Some(_)) => wildcard(&pattern[1..], &name[1..]),
        (Some(p), Some(n)) => p == n && wildcard(&pattern[1..], &name[1..]),
        _ => false,
    }
}

impl Ignore {
    pub fn load(root: &Path) -> Self {
        let patterns = fs::read_to_string(root.join(IGNORE_FILE))
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| match l.strip_suffix('/') {
                Some(dir) => (dir.to_owned(), true),
                None => (l.to_owned(), false),
            })
            .collect();
        Self { patterns }
    }
    pub fn ignored(&self, name: &str, directory: bool) -> bool {
        if name.starts_with('.') || (directory && name == "node_modules") {
            return true;
        }
        let name: Vec<char> = name.chars().collect();
        self.patterns.iter().any(|(p, dir_only)| {
            (!dir_only || directory) && wildcard(&p.chars().collect::<Vec<_>>(), &name)
        })
    }
}

pub fn children(dir: &Path, depth: usize, ignore: &Ignore) -> Result<Vec<Entry>> {
    let mut out = vec![];
    // Entries that vanish or cannot be inspected while listing are skipped.
    for item in fs::read_dir(dir)?.flatten() {
        let Ok(kind) = item.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        let name = item.file_name();
        if ignore.ignored(&name.to_string_lossy(), kind.is_dir()) {
            continue;
        }
        if kind.is_dir() || kind.is_file() && markdown(&item.path()) {
            out.push(Entry {
                path: item.path(),
                depth,
                directory: kind.is_dir(),
            });
        }
    }
    out.sort_by(|a, b| {
        b.directory.cmp(&a.directory).then_with(|| {
            a.path
                .file_name()
                .unwrap()
                .to_ascii_lowercase()
                .cmp(&b.path.file_name().unwrap().to_ascii_lowercase())
        })
    });
    Ok(out)
}

/// Like `children`, but an unreadable subfolder (for example one protected by the
/// system) counts as empty so it does not hide the rest of the tree. Only the root
/// itself must be readable.
fn readable_children(dir: &Path, depth: usize, ignore: &Ignore) -> Result<Vec<Entry>> {
    match children(dir, depth, ignore) {
        Err(_) if depth > 0 => Ok(vec![]),
        result => result,
    }
}

pub fn scan(root: &Path, expanded: &HashSet<PathBuf>) -> Result<Vec<Entry>> {
    fn walk(
        path: &Path,
        depth: usize,
        expanded: &HashSet<PathBuf>,
        ignore: &Ignore,
        out: &mut Vec<Entry>,
    ) -> Result<()> {
        if depth > 64 {
            return Ok(());
        }
        for item in readable_children(path, depth, ignore)? {
            let descend = item.directory && expanded.contains(&item.path);
            let path = item.path.clone();
            out.push(item);
            if descend {
                walk(&path, depth + 1, expanded, ignore, out)?;
            }
        }
        Ok(())
    }
    let mut out = vec![];
    walk(root, 0, expanded, &Ignore::load(root), &mut out)?;
    Ok(out)
}

fn walk_all(root: &Path, directories: bool) -> Result<Vec<PathBuf>> {
    fn walk(
        dir: &Path,
        depth: usize,
        ignore: &Ignore,
        directories: bool,
        out: &mut Vec<PathBuf>,
    ) -> Result<()> {
        if depth > 64 {
            return Ok(());
        }
        for item in readable_children(dir, depth, ignore)? {
            if item.directory {
                if directories {
                    out.push(item.path.clone());
                }
                walk(&item.path, depth + 1, ignore, directories, out)?;
            } else if !directories {
                out.push(item.path);
            }
        }
        Ok(())
    }
    let mut out = vec![];
    walk(root, 0, &Ignore::load(root), directories, &mut out)?;
    Ok(out)
}

pub fn all_notes(root: &Path) -> Result<Vec<PathBuf>> {
    walk_all(root, false)
}

/// The root and every visible folder below it.
pub fn all_folders(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = vec![root.to_path_buf()];
    out.extend(walk_all(root, true)?);
    Ok(out)
}

/// Whether two paths name the same file, e.g. `a.md` and `A.md` on a
/// case-insensitive file system.
pub fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (fs::metadata(a), fs::metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        match (a.canonicalize(), b.canonicalize()) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        }
    }
}

pub fn validated_name(value: &str, is_note: bool) -> Result<String> {
    let name = value.trim();
    if name.is_empty() || name.chars().any(|c| c.is_control()) || name.contains(['/', '\\']) {
        bail!("请输入单个文件名，不能包含路径分隔符");
    }
    let mut parts = Path::new(name).components();
    if !matches!(parts.next(), Some(Component::Normal(_))) || parts.next().is_some() {
        bail!("文件名无效");
    }
    if name.starts_with('.') {
        bail!("以 . 开头的名称会被隐藏，请换一个名称");
    }
    Ok(if is_note && !markdown(Path::new(name)) {
        format!("{name}.md")
    } else {
        name.to_string()
    })
}
