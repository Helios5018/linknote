use std::{fs, process::Command};
use tempfile::TempDir;

fn run(home: &TempDir, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_linknote"))
        .args(args)
        .env("HOME", home.path())
        .current_dir(home.path())
        .output()
        .unwrap()
}

#[test]
fn informational_and_invalid_arguments_do_not_create_notebook() {
    let home = TempDir::new().unwrap();
    for args in [vec!["--help"], vec!["open", "--help"], vec!["--version"]] {
        assert!(run(&home, &args).status.success());
    }
    for args in [
        vec!["open", "one", "two"],
        vec!["--help", "extra"],
        vec!["--unknown"],
        vec!["open", "--"],
    ] {
        assert!(!run(&home, &args).status.success());
    }
    assert!(!home.path().join("Note").exists());
}

#[test]
fn default_directory_collision_preserves_existing_file() {
    let home = TempDir::new().unwrap();
    let note = home.path().join("Note");
    fs::write(&note, "keep me").unwrap();
    let output = run(&home, &[]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("无法创建默认笔记目录"));
    assert_eq!(fs::read_to_string(note).unwrap(), "keep me");
}

#[test]
fn explicit_paths_and_literal_option_names_reach_terminal_validation() {
    let home = TempDir::new().unwrap();
    for name in ["open", "folder with spaces", "--help"] {
        fs::create_dir(home.path().join(name)).unwrap();
    }
    fs::write(home.path().join("中文 笔记.md"), "# Note").unwrap();
    for args in [
        vec!["open"],
        vec!["open", "folder with spaces"],
        vec!["open", "中文 笔记.md"],
        vec!["中文 笔记.md"],
        vec!["./open"],
        vec!["--", "open"],
        vec!["open", "--", "--help"],
    ] {
        let output = run(&home, &args);
        assert!(String::from_utf8_lossy(&output.stderr).contains("请在交互终端中运行"));
    }
    assert!(!home.path().join("Note").exists());
}
