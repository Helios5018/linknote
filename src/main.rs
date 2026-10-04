use anyhow::{Context, Result, bail};
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    },
    execute,
};
use linknote::app::App;
use std::{
    io::{self, IsTerminal},
    path::PathBuf,
    time::{Duration, Instant},
};

/// Request extended keys without a blocking capability probe. Unsupported
/// terminals ignore the sequence and continue to send legacy Ctrl keys.
#[cfg(unix)]
struct KeyboardMode;

#[cfg(unix)]
impl KeyboardMode {
    fn enable() -> io::Result<Self> {
        use crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
        execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        Ok(Self)
    }
}

#[cfg(unix)]
impl Drop for KeyboardMode {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), crossterm::event::PopKeyboardEnhancementFlags);
    }
}

fn main() -> Result<()> {
    let Some(path) = startup_path()? else {
        return Ok(());
    };
    let path = path.canonicalize().context("路径不存在或无法访问")?;
    let initial = path.is_file().then(|| path.clone());
    if initial.is_some() && !linknote::tree::markdown(&path) {
        bail!("只支持 .md / .markdown 文件");
    }
    let root = if initial.is_some() {
        path.parent().unwrap().to_path_buf()
    } else {
        path
    };
    if !root.is_dir() {
        bail!("请输入目录或 Markdown 文件");
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("请在交互终端中运行 linknote；用 --help 查看用法");
    }
    let mut app = App::new(root, initial)?;
    let mut terminal = ratatui::init();
    let result = (|| -> Result<()> {
        app.configure_images(linknote::images::picker());
        #[cfg(unix)]
        let _keyboard = KeyboardMode::enable()?;
        execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
        while !app.quit {
            app.tick();
            terminal.draw(|f| app.draw(f))?;
            if event::poll(Duration::from_millis(50))? {
                // Handle everything already queued before drawing again, so held keys
                // and fast input do not wait for one frame each.
                let deadline = Instant::now() + Duration::from_millis(30);
                loop {
                    app.event(event::read()?);
                    if app.quit || Instant::now() >= deadline || !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
        }
        Ok(())
    })();
    let cleanup = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    if result.is_err() {
        app.emergency_journal();
    }
    result?;
    cleanup?;
    Ok(())
}

fn startup_path() -> Result<Option<PathBuf>> {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    let open = first.as_deref() == Some(std::ffi::OsStr::new("open"));
    let argument = if open { args.next() } else { first };
    let literal = argument.as_deref() == Some(std::ffi::OsStr::new("--"));
    let argument = if literal {
        Some(args.next().context("-- 后需要一个目录或 Markdown 文件")?)
    } else {
        argument
    };
    if args.next().is_some() {
        bail!("只接受一个目录或 Markdown 文件；使用 --help 查看帮助");
    }
    if literal {
        return Ok(argument.map(PathBuf::from));
    }
    let path = match argument {
        Some(arg) if arg == "--help" || arg == "-h" => {
            println!(
                "LinkNote — 本地 Markdown 笔记\n\n用法:\n  linknote                       打开 ~/Note（不存在时自动创建）\n  linknote open                  打开当前目录\n  linknote open <目录或 Markdown 文件>\n  linknote <目录或 Markdown 文件>  兼容原有用法\n\nShell 别名：ln = linknote，lno = linknote open\n路径与命令或选项同名时，请使用 ./ 前缀或 -- 分隔。\n\n打开即编辑，停止输入后自动保存。F1 查看快捷键，F4 打开设置，Ctrl+Q 退出。\n支持 UTF-8、中文、鼠标和嵌套目录。\n\n环境变量：\n  LINKNOTE_STATE_DIR      光标记录与恢复草稿目录\n  LINKNOTE_CONFIG_DIR     设置 settings.json 与快捷键 keys.json 所在目录\n  LINKNOTE_REMOTE_IMAGES  设为 1 时自动加载所有网络图片\n  LINKNOTE_TRASH_DIR      删除时移到此目录，而不是系统废纸篓\n  LINKNOTE_OPENER         打开网址和非 Markdown 文件的程序"
            );
            return Ok(None);
        }
        Some(arg) if arg == "--version" || arg == "-V" => {
            println!("linknote {}", env!("CARGO_PKG_VERSION"));
            return Ok(None);
        }
        Some(arg) if arg.to_string_lossy().starts_with('-') => {
            bail!("未知选项：{}；使用 --help 查看帮助", arg.to_string_lossy());
        }
        Some(arg) => PathBuf::from(arg),
        None if open => std::env::current_dir().context("无法读取当前目录")?,
        None => {
            let home = directories::BaseDirs::new().context("无法确定用户主目录")?;
            let path = home.home_dir().join("Note");
            std::fs::create_dir_all(&path)
                .with_context(|| format!("无法创建默认笔记目录 {}", path.display()))?;
            path
        }
    };
    Ok(Some(path))
}
