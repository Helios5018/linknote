#!/usr/bin/env python3
"""Real PTY integration tests. No third-party Python packages, no personal notes touched."""
import errno
import atexit
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import zlib
import subprocess
import sys
import tempfile
import termios
import time

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get("LINKNOTE_BIN", ROOT / "target/debug/linknote"))


class Terminal:
    def __init__(self, path, state, width=100, height=30, protocol="halfblocks", env_extra=None, args=None, cwd=None):
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
        env = dict(os.environ, TERM="xterm-256color", COLORTERM="truecolor", LINKNOTE_STATE_DIR=str(state),
                   LINKNOTE_CONFIG_DIR=str(state / "config"), LINKNOTE_IMAGE_PROTOCOL=protocol,
                   LINKNOTE_CLIPBOARD="internal", **(env_extra or {}))
        env.pop("NO_COLOR", None)  # tool runners may disable ANSI colors; test the actual color protocol
        command = [str(BIN), *(args if args is not None else ["open", str(path)])]
        self.process = subprocess.Popen(command, stdin=slave, stdout=slave,
                                        stderr=slave, env=env, cwd=cwd, start_new_session=True)
        atexit.register(self.cleanup)
        os.close(slave)
        self.output = b""
        self.query_tail = b""
        deadline = time.monotonic() + 5
        while b"linknote" not in self.output.lower() and self.process.poll() is None and time.monotonic() < deadline:
            self.wait(0.05)
        assert b"linknote" in self.output.lower(), repr(self.output[-2000:])

    def wait(self, seconds=0.2):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            if select.select([self.master], [], [], min(0.03, max(0, end-time.monotonic())))[0]:
                try:
                    chunk = os.read(self.master, 65536)
                    if not chunk:
                        break
                    self.output += chunk
                    # Emulate the DSR response a real terminal/tmux sends. Native
                    # protocol tests otherwise leave the capability reader waiting
                    # on stdin, where it would consume the first editing key.
                    if b"\x1b[5n" in self.query_tail + chunk:
                        os.write(self.master, b"\x1b[0n")
                    self.query_tail = chunk[-3:]
                except OSError as e:
                    if e.errno == errno.EIO:
                        break
                    raise

    def send(self, value, pause=0.12):
        os.write(self.master, value.encode() if isinstance(value, str) else value)
        self.wait(pause)

    def paste(self, text, pause=0.12):
        self.send("\x1b[200~" + text + "\x1b[201~", pause)

    def resize(self, width, height):
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
        os.kill(self.process.pid, signal.SIGWINCH)
        self.wait()

    def text(self):
        return re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", self.output).decode(errors="replace")

    def styled_runs(self):
        """Observe actual SGR output, not only the app's internal style flags."""
        bold, foreground = False, None
        runs = []
        for part in re.split(r"(\x1b\[[0-?]*[ -/]*[@-~])", self.output.decode(errors="replace")):
            if part.startswith("\x1b["):
                if not part.endswith("m"):
                    continue
                params = [int(p or 0) for p in part[2:-1].split(";")]
                i = 0
                while i < len(params):
                    p = params[i]
                    if p == 0:
                        bold, foreground = False, None
                    elif p == 1:
                        bold = True
                    elif p == 22:
                        bold = False
                    elif p in (38, 48) and i + 4 < len(params) and params[i + 1] == 2:
                        if p == 38:
                            foreground = tuple(params[i + 2:i + 5])
                        i += 4
                    elif p == 39:
                        foreground = None
                    i += 1
            else:
                runs.append((part, bold, foreground))
        return runs

    def expect(self, condition, label, timeout=4):
        deadline = time.monotonic() + timeout
        while not condition() and time.monotonic() < deadline:
            self.wait(0.05)
        assert condition(), label + "\n" + self.text()[-2200:]

    def quit(self):
        self.send(b"\x11", 0.3)
        # Native image teardown can still write to the PTY. Drain it while waiting
        # so a full output buffer cannot prevent an otherwise clean process exit.
        self.expect(lambda: self.process.poll() is not None, "process exits after Ctrl+Q", timeout=5)
        assert self.process.returncode == 0, self.output.decode(errors="replace")[-1500:]
        os.close(self.master)

    def kill(self):
        self.process.kill()
        self.process.wait(timeout=3)
        os.close(self.master)

    def cleanup(self):
        if self.process.poll() is None:
            self.kill()


def run():
    if not BIN.exists():
        raise SystemExit("先运行 cargo build")
    with tempfile.TemporaryDirectory(prefix="linknote-smoke-") as temp:
        base = Path(temp)
        home = base / "home"
        home.mkdir()
        elsewhere = base / "elsewhere"
        elsewhere.mkdir()
        (elsewhere / "current.md").write_text("# Current directory\n")
        # No arguments must create ~/Note and save new notes there, regardless of cwd.
        t = Terminal(None, base / "default-state", args=[], cwd=elsewhere,
                     env_extra={"HOME": str(home)})
        assert (home / "Note").is_dir()
        t.send("a")  # empty tree: create
        t.paste("default-note")
        t.send(b"\r")
        t.paste("Saved in home notebook")
        t.expect(lambda: (home / "Note/default-note.md").exists()
                 and "Saved in home notebook" in (home / "Note/default-note.md").read_text(),
                 "default launch creates and edits notes in ~/Note")
        t.quit()
        t = Terminal(None, base / "default-state", args=[], cwd=base,
                     env_extra={"HOME": str(home)})
        t.expect(lambda: "default-note.md" in t.text(), "another cwd restores the same notebook")
        t.quit()
        t = Terminal(None, base / "current-state", args=["open"], cwd=elsewhere,
                     env_extra={"HOME": str(home)})
        t.expect(lambda: "current.md" in t.text(), "open uses cwd")
        t.paste("Edited here: ")
        t.expect(lambda: (elsewhere / "current.md").read_text().startswith("Edited here: "),
                 "open edits the current directory's note")
        t.quit()
        print("PASS: default ~/Note creation, cross-directory launch, persistence, open current directory")
        # Real CSI-u events exercise the terminal decoder, not just App::key.
        # A private clipboard prevents tests from replacing the user's clipboard.
        command_root = base / "command-notes"
        command_root.mkdir()
        command_note = command_root / "command.md"
        command_source = "原文中文 👨‍👩‍👧‍👦\n第二行"
        command_note.write_text(command_source)
        command_state = base / "command-state"
        if os.uname().sysname != "Darwin":
            (command_state / "config").mkdir(parents=True)
            (command_state / "config/keys.json").write_text(json.dumps({"keys": {
                "select_all": ["cmd+a", "ctrl+a"], "copy": ["cmd+c", "ctrl+c"],
                "cut": ["cmd+x", "ctrl+x"], "paste": ["cmd+v", "ctrl+v"],
                "undo": ["cmd+z", "ctrl+z"], "redo": ["cmd+shift+z", "ctrl+y"],
            }}))
        t = Terminal(command_note, command_state)
        assert b"\x1b[>1u" in t.output, "enhanced keyboard mode is requested"
        def command_key(letter, shift=False):
            t.send(f"\x1b[{ord(letter)};{10 if shift else 9}u")
        command_key("a")
        command_key("c")
        command_key("x")
        t.expect(lambda: command_note.read_text() == "", "Command+X cuts selection")
        command_key("z")
        t.expect(lambda: command_note.read_text() == command_source, "Command+Z restores cut")
        command_key("z", shift=True)
        t.expect(lambda: command_note.read_text() == "", "Command+Shift+Z redoes cut")
        command_key("v")
        t.expect(lambda: command_note.read_text() == command_source, "Command+V pastes once")
        t.send(b"\x17r")  # focus tree, rename
        command_key("a")
        t.paste("改名")
        command_key("z")
        command_key("z", shift=True)
        t.send(b"\r")
        command_note = command_root / "改名.md"
        t.expect(command_note.exists, "Command editing in the rename prompt")
        t.send(b"\t")  # resume editing
        t.send(b"\x06")
        t.paste("原文中文")
        command_key("a")
        command_key("c")
        t.send(b"\t")
        command_key("v")
        command_key("a")
        t.paste("替换")
        command_key("z")
        command_key("z", shift=True)
        t.send(b"\x12")  # Replace all in note, independently of input undo.
        t.expect(lambda: command_note.read_text() == command_source.replace("原文中文", "替换"),
                 "find/replace inputs use the same clipboard and undo keys")
        command_key("z")
        t.expect(lambda: command_note.read_text() == command_source, "note undo survives prompt editing")
        command_key("k")  # Unbound Command key must never insert a letter.
        t.send(b"\x1b[97;5u")  # Enhanced Ctrl+A remains available.
        t.paste("兼容 Ctrl")
        t.send(b"\x1a")  # Legacy Ctrl+Z also remains available.
        t.expect(lambda: command_note.read_text() == command_source, "Ctrl compatibility")
        t.quit()
        assert b"\x1b[<1u" in t.output, "enhanced keyboard mode is restored on exit"
        print("PASS: CSI-u Command editing, clipboard, prompt undo/redo, Ctrl fallback, keyboard cleanup")

        notes = base / "notes"
        state = base / "state"
        notes.mkdir()
        (notes / "nested").mkdir()
        (notes / "empty").mkdir()
        (notes / "hidden.txt").write_text("not a note")
        note = notes / "01.md"
        note.write_text("# 标题\n\n第一段 **粗体**。\n\n第二段。\n")
        (notes / "nested/02.MD").write_text("# Nested\n")
        t = Terminal(notes, state)
        try:
            assert b"linknote" in t.output.lower()
            assert b"hidden.txt" not in t.output
            # Default focus is the editor, and no explicit save is sent.
            t.paste("开头中文👨‍👩‍👧‍👦\n", pause=0.8)
            assert note.read_text().startswith("开头中文👨‍👩‍👧‍👦\n# 标题")
            t.send(b"\x1a", 0.7)  # undo whole paste
            assert note.read_text().startswith("# 标题")
            t.send(b"\x19", 0.7)  # redo
            assert note.read_text().startswith("开头中文")
            # Search selects the raw source text; typing replaces that selection.
            t.send(b"\x06")
            t.paste("第一段")
            t.send(b"\r")
            t.paste("替换段落", pause=0.7)
            assert "替换段落 **粗体**" in note.read_text()
            # Save pending edits when opening a nested note from the picker.
            t.paste("追加")
            t.send(b"\x10")
            t.paste("02.MD")
            t.send(b"\r")
            assert "追加" in note.read_text()
            t.paste("Nested edit\n", pause=0.7)
            assert (notes / "nested/02.MD").read_text().startswith("Nested edit")
            # Create a note in the active directory, then rename it.
            t.send(b"\x17a")
            t.paste("新建笔记")
            t.send(b"\r")
            t.paste("# 新笔记\n\n正文。", pause=0.7)
            created = notes / "nested/新建笔记.md"
            t.expect(lambda: created.exists() and created.read_text() == "# 新笔记\n\n正文。", "new note autosave")
            t.send(b"\x17r")  # tree rename
            t.send(b"\x15")
            t.paste("重命名")
            t.send(b"\r")
            renamed = notes / "nested/重命名.md"
            assert renamed.exists() and not created.exists()
            t.send(b"\t")
            # Narrow and wide resizes exercise layout and cursor placement.
            t.resize(44, 16)
            t.send(b"\x1b[F")
            t.paste("窄屏输入", pause=0.7)
            assert "窄屏输入" in renamed.read_text()
            t.resize(120, 32)
            # External reload of a clean buffer.
            renamed.write_text("外部新内容\n")
            t.wait(1.3)
            assert "外部新内容" in t.text()
            # Conflict: local dirty edit followed immediately by an external write.
            t.paste("本地版本", pause=0.05)
            renamed.write_text("外部冲突版本\n")
            t.wait(0.8)
            assert renamed.read_text() == "外部冲突版本\n"
            t.send(b"\x12")  # conflict resolution
            t.send(b"\r")   # keep both
            copies = list((notes / "nested").glob("重命名.local-*.md"))
            assert len(copies) == 1 and "本地版本" in copies[0].read_text()
            assert renamed.read_text() == "外部冲突版本\n"
            # New empty folder, nested note, and renaming its parent while the note is open.
            t.send(b"\x17a")
            t.paste("子目录/")
            t.send(b"\r")
            assert (notes / "nested/子目录").is_dir()
            t.send("a")
            t.paste("deep")
            t.send(b"\r")
            t.paste("deep contents", pause=0.7)
            t.send(b"\x17")  # focus tree on deep.md
            t.send(b"\x1b[D")  # select parent folder
            t.send("r")
            t.send(b"\x15")
            t.paste("改名目录")
            t.send(b"\r")
            t.send(b"\x17")  # editor
            t.paste(" after move", pause=0.7)
            assert (notes / "nested/改名目录/deep.md").read_text() == "deep contents after move"
            assert not (notes / "nested/子目录").exists()
            t.quit()
        except Exception:
            print("TERMINAL TAIL:", t.text()[-2500:])
            if t.process.poll() is None:
                t.kill()
            raise
        print("PASS: editor, Unicode paste, undo/redo, find, nested picker, autosave, file/folder create+rename, resize, reload, conflict")

        # A real sidebar click must retain focus so plain a/A opens creation.
        create_root = base / "create-notes"
        create_root.mkdir()
        (create_root / "a.md").write_text("original")
        t = Terminal(create_root, base / "create-state")
        t.send(b"\x1b[<0;6;4M\x1b[<0;6;4m")
        t.send("a")
        t.paste("new-note")
        t.send(b"\r")
        t.paste("created via mouse and a")
        t.expect(lambda: (create_root / "new-note.md").exists()
                 and (create_root / "new-note.md").read_text() == "created via mouse and a",
                 "sidebar file click keeps focus for a and new note autosaves")
        t.send(b"\x1b[<0;6;16M\x1b[<0;6;16m")  # blank sidebar
        t.send("A")
        t.paste("资料/")
        t.send(b"\r")
        t.expect(lambda: (create_root / "资料").is_dir(), "uppercase A creates folder via slash")
        t.send("a")
        t.paste("data.xxx")
        t.send(b"\r")
        t.expect(lambda: "只支持新建" in t.text(), "non-Markdown extension is rejected")
        assert not (create_root / "资料/data.xxx").exists()
        assert not (create_root / "资料/data.xxx.md").exists()
        t.send(b"\x01")
        t.paste("data.md")
        t.send(b"\r")
        t.expect(lambda: (create_root / "资料/data.md").is_file(), "corrected name creates Markdown")
        t.send(b"\x17a")
        t.paste("data.md")
        t.send(b"\r")
        t.expect(lambda: "新建失败" in t.text(), "duplicate creation stays in prompt")
        t.send(b"\x1b")
        t.send("r")
        t.send(b"\x01")
        t.paste("renamed")
        t.send(b"\r")
        t.expect(lambda: (create_root / "资料/renamed.md").exists()
                 and not (create_root / "资料/data.md").exists(), "r renames selected note")
        t.send("c")
        t.send(b"\t")
        t.send(b"\x16")
        t.expect(lambda: (create_root / "资料/renamed.md").read_text()
                 == str((create_root / "资料/renamed.md").resolve()), "c copies absolute path to clipboard")
        assert (create_root / "a.md").read_text() == "original"
        t.quit()
        print("PASS: sidebar focus, a/A create, Markdown only, slash folder, collision, prompt correction, r rename, c copy path")

        delete_root = base / "delete-notes"
        delete_root.mkdir()
        (delete_root / "a.md").write_text("keep until confirmed")
        (delete_root / "folder").mkdir()
        (delete_root / "folder/child.md").write_text("folder content")
        delete_trash = base / "delete-trash"
        t = Terminal(delete_root / "a.md", base / "delete-state",
                     env_extra={"LINKNOTE_TRASH_DIR": str(delete_trash)})
        t.send(b"\x17")
        t.send("dd")
        t.expect(lambda: "废纸篓" in t.text(), "d opens trash confirmation")
        assert (delete_root / "a.md").exists()
        t.send(b"\x1b")
        assert (delete_root / "a.md").exists() and not delete_trash.exists()
        t.send("D")
        assert (delete_root / "a.md").exists()
        t.send(b"\r")
        t.expect(lambda: (delete_trash / "a.md").exists() and not (delete_root / "a.md").exists(),
                 "Shift+D only trashes after Enter")
        assert (delete_trash / "a.md").read_text() == "keep until confirmed"
        t.send("D")  # remaining selection is the folder
        assert (delete_root / "folder/child.md").exists()
        t.send(b"\r")
        t.expect(lambda: (delete_trash / "folder/child.md").exists(), "folder contents move to trash")
        assert (delete_trash / "folder/child.md").read_text() == "folder content"
        t.quit()
        print("PASS: d/Shift+D trash confirmation, repeated key safety, cancel, preserved file/folder contents")

        # Crash recovery in a real process: kill after journal, before debounce save.
        crash = notes / "crash.md"
        crash.write_text("原文")
        t = Terminal(crash, state)
        t.paste("恢复的草稿", pause=0.23)
        t.kill()
        assert crash.read_text() == "原文"
        drafts = list((state / "recovery").glob("*.json"))
        assert any("恢复的草稿" in json.loads(p.read_text())["text"] for p in drafts)
        t = Terminal(crash, state)
        assert "发现未完成的草稿" in t.text()
        t.send(b"\r", 0.8)
        assert crash.read_text() == "恢复的草稿原文"
        t.quit()
        print("PASS: SIGKILL recovery, restored autosave, clean exit")

        # Mouse click and drag use actual SGR mouse events in terminal cell coordinates.
        mouse = notes / "mouse.md"
        mouse.write_text("first **bold** end\n\n中文段落\n\nlast\n")
        t = Terminal(mouse, state)
        t.send(b"\x1b[<0;30;7M\x1b[<0;30;7m")
        t.paste("X", pause=0.7)
        assert "中X文段落" in mouse.read_text(), mouse.read_text()
        t.send(b"\x1a", 0.7)
        t.send(b"\x1b[<0;28;7M")
        t.send(b"\x1b[<32;32;7M\x1b[<0;32;7m")
        t.paste("替换", pause=0.7)
        assert "替换段落" in mouse.read_text(), mouse.read_text()
        assert "中文" not in mouse.read_text()
        t.quit()
        print("PASS: mouse click in rendered Chinese paragraph, drag selection, replace, autosave")

        # Sidebar buttons and Ctrl+B share the toggle, including narrow windows.
        t = Terminal(mouse, state)
        before = mouse.read_text()
        t.send(b"\x1b[<0;24;3M\x1b[<0;24;3m")
        t.expect(lambda: ">" in t.text(), "sidebar collapse button")
        t.send(b"\x1b[<0;3;5M\x1b[<0;3;5m")
        t.paste("FULL ", pause=0.7)
        assert mouse.read_text() == "FULL " + before
        t.send(b"\x1a", 0.7)
        t.output = b""
        t.send(b"\x1b[<0;2;3M\x1b[<0;2;3m")
        t.expect(lambda: "<" in t.text(), "sidebar expand button")
        t.send(b"\x02")  # Ctrl+B: collapse
        t.send(b"\x02")  # Ctrl+B: expand
        t.resize(44, 16)
        t.output = b""
        t.send(b"\x1b[<0;2;3M\x1b[<0;2;3m")
        t.expect(lambda: "<" in t.text(), "narrow sidebar can expand")
        t.send(b"\x02")
        t.send(b"\x1b[<0;3;5M\x1b[<0;3;5m")
        t.paste("NARROW ", pause=0.7)
        assert mouse.read_text() == "NARROW " + before
        t.send(b"\x1a", 0.7)
        t.quit()
        assert mouse.read_text() == before
        print("PASS: sidebar collapse/expand buttons, Ctrl+B, narrow window, editor hit mapping and note safety")

        # Divider drag changes geometry without selecting or editing note text.
        t = Terminal(mouse, state)
        before = mouse.read_text()
        t.send(b"\x1b[<0;25;10M")
        t.send(b"\x1b[<32;40;10M\x1b[<0;40;10m")

        def saved_width():
            return json.loads(next(state.glob("*.session.json")).read_text()).get("sidebar_width")

        t.expect(lambda: saved_width() == 40, "dragged sidebar width persisted")
        assert mouse.read_text() == before
        t.send(b"\x1b[44;5u")  # Ctrl+, settings
        t.send(b"\x1b[B\x1b[B\x1b[D")  # width: narrower
        t.expect(lambda: saved_width() == 38, "settings narrows sidebar")
        t.send(b"\x1b[C")  # width: wider
        t.expect(lambda: saved_width() == 40, "settings widens sidebar")
        t.send(b"\x1b")
        t.quit()
        t = Terminal(mouse, state)
        # At width 40 the editor text starts at terminal column 43 (1-based).
        t.send(b"\x1b[<0;43;5M\x1b[<0;43;5m")
        t.paste("RESIZED ")
        t.expect(lambda: mouse.read_text() == "RESIZED " + before, "reopened geometry and editor hit map")
        # Clamp both edges, then ensure a narrow terminal does not erase the preference.
        t.send(b"\x1b[<0;40;10M\x1b[<32;99;10M\x1b[<0;99;10m")
        t.expect(lambda: saved_width() == 68, "right pane keeps 32 columns")
        t.send(b"\x1b[<0;68;10M\x1b[<32;1;10M\x1b[<0;1;10m")
        t.expect(lambda: saved_width() == 20, "left pane keeps 20 columns")
        t.resize(44, 16)
        assert saved_width() == 20
        t.resize(100, 30)
        t.send(b"\x1b[<0;23;5M\x1b[<0;23;5m")
        t.paste("NARROW ")
        t.expect(lambda: mouse.read_text() == "NARROW RESIZED " + before, "restored geometry after terminal resize")
        t.quit()
        print("PASS: divider drag, keyboard width, persistence, clamping, narrow terminal and click mapping")

        # A fresh root uses the default split; clicking a drawn cell must edit its source.
        table_root = base / "table-notes"
        table_root.mkdir()
        table_note = table_root / "table.md"
        table_source = "| 操作 | 快捷键 |\n| --- | --- |\n| 新建 | a |\n| 帮助 | F1 |\n\n- 一级\n  - 二级\n    - 三级\n\n1. 第一\n   1. 子项\n\nend\n"
        table_note.write_text(table_source)
        t = Terminal(table_root, state)
        t.send(b"\x1b[1;5F")  # Ctrl+End leaves the table rendered
        t.expect(lambda: "╭" in t.text() and "╰" in t.text(), "table borders rendered")
        assert t.text().count("├") >= 2, "header and data rows need horizontal separators"
        assert "◦ 二级" in t.text() and "▪ 三级" in t.text()
        assert "1.1. 子项" in t.text()
        t.output = b""
        t.send(b"\x1b[<0;30;10M\x1b[<0;30;10m")  # second record, below the new horizontal rule
        t.send(b"\x1b[1;5H")  # Ctrl+Home: source table at top, with its own row rules
        t.send(b"\x1b[B\x1b[B\x1b[B")  # header -> delimiter -> first record -> second record
        t.expect(lambda: "├" in t.text(), "source table keeps horizontal separators")
        t.send(b"\x1b[3~")  # delete only the real opening pipe, not a decorative rule
        t.send(b"\x1a")
        # Locate the original word again so source layout padding cannot affect typing.
        t.send(b"\x06")
        t.paste("帮助")
        t.send(b"\r")
        t.send(b"\x1b[D")  # clear selection at its start
        t.paste("改")
        t.expect(lambda: table_note.read_text() == table_source.replace("| 帮助 |", "| 改帮助 |"), "click rendered cell edits correct source after horizontal separator")
        t.send(b"\x1b[1;5F")
        t.resize(64, 30)
        t.send(b"\x1b[44;5u\x1b[B\x1b[B\x1b[C")
        t.send(b"\x1b")
        t.quit()
        print("PASS: drawn table, nested list hierarchy, table click-to-edit, resize and save")

        # The real renderer loads an image relative to a nested note, and its canvas
        # remains clickable. PNG fixture is generated with the standard library.
        image_root = base / "image-notes"
        nested = image_root / "nested"
        nested.mkdir(parents=True)
        def chunk(kind, data):
            return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
        pixels = b"".join(b"\0" + b"".join(bytes((x * 3, y * 3, 100)) for x in range(80)) for y in range(80))
        png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 80, 80, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(pixels)) + chunk(b"IEND", b"")
        (nested / "preview.png").write_bytes(png)
        image_note = nested / "picture.md"
        image_source = "# Picture\n\n![示例](preview.png)\n\n**采取行动（Action）：**继续记录。\n\nend\n"
        image_note.write_text(image_source)
        t = Terminal(image_root, state)
        t.send(b"\x10")
        t.paste("picture.md")
        t.output = b""
        t.send(b"\r")
        t.expect(lambda: any(c in t.text() for c in "▀▄"), "local image is drawn in the terminal")
        t.expect(lambda: "🔍" in t.text(), "image preview exposes the magnifier button")
        assert "nested/picture.md" not in t.text(), "editor title shows filename only"
        assert "**采取行动" not in t.text(), "CJK punctuation bold is rendered"
        runs = t.styled_runs()
        assert any("采" in text and bold and fg == (242, 239, 226) for text, bold, fg in runs), "CJK strong text uses the highlight color and SGR 1"
        assert any("继" in text and not bold and fg == (207, 216, 227) for text, bold, fg in runs), "following text restores the normal foreground and weight"
        assert image_note.read_text() == image_source
        t.send(b"\x1b[<0;30;9M\x1b[<0;30;9m")
        t.expect(lambda: "![示例](preview.png)" in t.text(), "image canvas click reveals Markdown")
        t.send(b"\x06")
        t.paste("preview.png")
        t.send(b"\r")
        t.paste("absent.png")
        t.send(b"\x1b[1;5F")
        t.expect(lambda: "图片无法加载" in t.text(), "missing images show a readable error")
        t.send(b"\x1a")
        t.output = b""
        t.send(b"\x1b[1;5F")
        t.expect(lambda: any(c in t.text() for c in "▀▄"), "restored image renders after undo")
        t.resize(64, 20)
        t.send(b"\x1b[44;5u\t\x1b[F\r")  # settings: reload images
        t.send(b"\x1b")
        t.quit()
        assert image_note.read_text() == image_source
        print("PASS: inline image, nested path, image click-to-edit, error/undo, resize, CJK bold, filename-only title")

        t = Terminal(image_root, state, protocol="kitty")
        t.send(b"\x1b[1;5F")
        t.expect(lambda: b"\x1b_G" in t.output, "native Kitty image protocol is emitted")
        t.quit()
        print("PASS: native Kitty graphics emission (pixel appearance depends on the host terminal)")

        # Features added after 0.1.0, through the real key sequences of a terminal.
        feat = base / "feature-notes"
        (feat / "归档").mkdir(parents=True)
        (feat / "a.md").write_text("# 首页\n\n见 [[目标]] 和 [二](b.md#第二节)\n\n- 一\n- 二\n")
        (feat / "b.md").write_text("# 一\n\n## 第二节\n\n独特词汇在这里\n")
        (feat / "归档/目标.md").write_text("目标笔记\n")
        trash = base / "trash"
        t = Terminal(feat / "a.md", state, env_extra={"LINKNOTE_TRASH_DIR": str(trash), "LINKNOTE_OPENER": "true"})
        assert "Ctrl+P 打开" not in t.text(), "bottom shortcut bar removed"
        # Full-text search (Ctrl+P, Tab) opens the match selected; typing replaces it.
        t.send(b"\x10\t")
        t.paste("独特词汇")
        t.expect(lambda: "b.md:5" in t.text(), "full-text search result listed")
        t.send(b"\r")
        t.paste("改", pause=0.8)
        t.expect(lambda: "改在这里" in (feat / "b.md").read_text(), "search hit opened and edited")
        # Follow a wiki link with F12, edit the target, come back with Shift+F12.
        t.send(b"\x10")
        t.paste("a.md")
        t.send(b"\r")
        t.send(b"\x06")
        t.paste("目标")
        t.send(b"\r")
        t.send(b"\x1b[24~")  # F12
        t.send(b"\x1b[1;5H")  # Ctrl+Home
        t.paste("X")
        t.expect(lambda: (feat / "归档/目标.md").read_text() == "X目标笔记\n", "wiki link opened the target note")
        t.send(b"\x1b[24;2~")  # Shift+F12
        # Tab nests a list item under the previous one.
        t.send(b"\x06")
        t.paste("- 二")
        t.send(b"\r")
        t.send(b"\x1b[C")  # collapse the selection
        t.send(b"\t")
        t.expect(lambda: (feat / "a.md").read_text().endswith("- 一\n  - 二\n"), "Tab indents the list item")
        # Move from the tree with m, then rename it changing only the letter case.
        t.send(b"\x17m")
        t.paste("归档")
        t.send(b"\r")
        t.expect(lambda: (feat / "归档/a.md").exists() and not (feat / "a.md").exists(), "m moved the note")
        t.send("r")
        t.send(b"\x15")
        t.paste("A.md")
        t.send(b"\r")
        t.expect(lambda: "A.md" in os.listdir(feat / "归档"), "case-only rename")
        # Delete from the tree goes to the trash directory after confirmation.
        t.send(b"\x1b[F")  # End: b.md is the last entry
        t.send("d")  # trash
        t.send(b"\r")
        t.expect(lambda: (trash / "b.md").exists() and not (feat / "b.md").exists(), "deleted to trash")
        # A conflicted note no longer blocks opening another one.
        t.send(b"\x17")  # editor
        t.paste("本地", pause=0.05)
        (feat / "归档/A.md").write_text("外部\n")
        t.send(b"\x10")
        t.paste("目标")
        t.send(b"\r")
        t.expect(lambda: "恢复草稿" in t.text(), "switched away from the conflicted note")
        assert (feat / "归档/A.md").read_text() == "外部\n"
        t.quit()
        print("PASS: no hint bar, full-text search, wiki link + back, list Tab, move, case rename, trash, leave conflict")

        # Mermaid: the actual binary emits graphics, edits the original fence,
        # The full-screen viewer isolates input, uses native graphics after zoom,
        # returns to the original selection and opens a local file on explicit O.
        opened_path = base / "opened-image.txt"
        opener = base / "image-opener"
        opener.write_text(f"#!{sys.executable}\nimport sys\nfrom pathlib import Path\nPath({str(opened_path)!r}).write_text(sys.argv[1])\n")
        opener.chmod(0o755)
        t = Terminal(image_note, base / "viewer-state", protocol="kitty", env_extra={"LINKNOTE_OPENER": str(opener)})
        t.send(b"\x06")
        t.paste("preview.png")
        t.send(b"\r")
        t.output = b""
        t.send(b"\x1b[21~")  # F10
        t.expect(lambda: "图片查看" in t.text(), "F10 opens the full-screen viewer")
        t.expect(lambda: b"\x1b_G" in t.output, "viewer renders native image", timeout=10)
        t.output = b""
        t.send(b"++")
        t.expect(lambda: b"\x1b_G" in t.output, "zoom emits a new native viewport", timeout=10)
        t.send(b"\x1b[C\x1b[B")
        t.send(b"\x1b[<0;50;10M\x1b[<32;42;8M\x1b[<0;42;8m")
        t.paste("must not edit")
        t.resize(60, 18)
        t.send(b"0o")
        t.expect(lambda: opened_path.exists(), "explicit O calls the system opener")
        assert Path(opened_path.read_text()).resolve() == (nested / "preview.png").resolve()
        t.send(b"\x1b")
        assert image_note.read_text() == image_source
        t.paste("absent.png")
        t.expect(lambda: "absent.png" in image_note.read_text(), "closing restores the selected reference")
        t.send(b"\x1a", 0.7)
        t.quit()
        assert image_note.read_text() == image_source
        print("PASS: full-screen image, F10, native zoom, pan/drag, resize, system opener, modal paste isolation and selection restoration")

        # updates its image after editing, and reports syntax errors in-place.
        mermaid_root = base / "mermaid"
        mermaid_root.mkdir()
        diagram = mermaid_root / "diagram.md"
        source = "# Diagram\n\n```mermaid\nflowchart LR\n A[Start] --> B[Done]\n```\n\nTail\n"
        diagram.write_text(source)
        t = Terminal(mermaid_root, base / "mermaid-state", protocol="kitty", env_extra={"LINKNOTE_OPENER": str(opener)})
        t.expect(lambda: b"\x1b_G" in t.output, "Mermaid emits native graphics", timeout=15)
        assert diagram.read_text() == source, "preview must not rewrite the Markdown"
        t.send(b"\x06")
        t.paste("Start")
        t.send(b"\r")
        t.output = b""
        t.send(b"\x1b[21~")
        t.expect(lambda: "图片查看" in t.text(), "Mermaid source opens its viewer")
        t.expect(lambda: b"\x1b_G" in t.output, "Mermaid vector viewport renders", timeout=15)
        t.output = b""
        t.send(b"++")
        t.expect(lambda: b"\x1b_G" in t.output, "Mermaid zoom rasterizes a new viewport", timeout=15)
        opened_path.unlink()
        t.send(b"o")
        t.expect(lambda: opened_path.exists(), "Mermaid opens a local vector page")
        exported = Path(opened_path.read_text())
        assert '<svg' in exported.read_text()
        assert 'Content-Security-Policy' in exported.read_text()
        t.send(b"\x1b")
        assert exported.exists(), "external page survives closing the viewer"
        t.paste("中文起点")
        t.expect(lambda: diagram.read_text() == source.replace("Start", "中文起点"), "diagram source autosaves")
        t.output = b""
        t.send(b"\x1b[1;5H")  # Ctrl+Home leaves the source block
        t.expect(lambda: b"\x1b_G" in t.output, "edited Mermaid renders again", timeout=15)
        t.resize(72, 22)
        t.send(b"\x1b[<65;45;14M")  # scroll through canvas
        t.send(b"\x1b[<64;45;14M")
        t.send(b"\x01")  # Ctrl+A
        broken = "# Invalid\n\n```mermaid\nflowchart LR\n A[unclosed\n```\n"
        t.paste(broken)
        t.send(b"\x1b[1;5H")
        # Ratatui keeps the unchanged "Mermaid" prefix and redraws only the suffix.
        t.expect(lambda: "无法渲染" in t.text(), "syntax error stays editable", timeout=15)
        t.expect(lambda: diagram.read_text() == broken, "invalid source is saved unchanged")
        t.quit()
        assert not exported.exists(), "generated external file is cleaned up at exit"
        print("PASS: Mermaid graphics, vector zoom, local external page, source editing, autosave, rerender, resize, scrolling, syntax error")

        # Unified settings: categories, persisted theme, keyboard capture, mouse
        # entry, and note safety through resize and a fresh process.
        prefs_root = base / "preferences"
        prefs_root.mkdir()
        pref_note = prefs_root / "note.md"
        pref_source = "# Settings demo\n\n**Bold** and normal text.\n"
        pref_note.write_text(pref_source)
        pref_state = base / "preferences-state"
        pref_path = pref_state / "config/settings.json"
        keys_path = pref_state / "config/keys.json"
        t = Terminal(prefs_root, pref_state)
        t.send(b"\x1b[44;5u")  # Ctrl+,
        t.expect(lambda: "整体主题" in t.text(), "settings opens with Ctrl+,")
        t.send(b"\x1b[C")
        t.expect(lambda: pref_path.exists() and json.loads(pref_path.read_text())["theme"] == "light", "light theme persists")
        t.send(b"\t")
        t.expect(lambda: "Mermaid 主题" in t.text(), "diagram settings category")
        t.send(b"\x1b[C")  # follow app -> dark
        t.expect(lambda: json.loads(pref_path.read_text())["mermaid_theme"] == "dark", "diagram preference persists")
        t.send(b"\t")
        t.send(b"\r")  # capture Help
        t.send(b"\x1b[44;5u")  # Ctrl+, conflicts
        t.expect(lambda: "已用于" in t.text(), "shortcut conflict is visible")
        t.send(b"\x1b[23~")  # F11
        t.expect(lambda: keys_path.exists() and json.loads(keys_path.read_text())["keys"]["help"] == ["F11"], "shortcut saves")
        t.send(b"\x1b")
        t.send(b"\x1b[23~")
        t.expect(lambda: "直接输入即可编辑" in t.text(), "new shortcut is immediately active")
        t.send(b"\x1b")
        t.send(b"\x1b[<0;94;1M")  # clickable top-right settings entry
        t.send(b"\x1b[<0;94;1m")
        t.resize(40, 12)
        t.send(b"\t")
        t.send(b"\x1b")
        assert pref_note.read_text() == pref_source
        t.quit()
        t = Terminal(prefs_root, pref_state)
        t.expect(lambda: b"48;2;247;248;250" in t.output, "app restores light background on restart")
        t.send(b"\x1b[23~")
        t.expect(lambda: "直接输入即可编辑" in t.text(), "shortcut survives restart")
        t.send(b"\x1b")
        t.quit()
        assert pref_note.read_text() == pref_source
        print("PASS: settings categories, app/diagram themes, shortcut capture/conflicts, mouse, resize, persistence, note safety")


if __name__ == "__main__":
    run()
