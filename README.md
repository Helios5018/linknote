<div align="center">

# LinkNote

在终端里写 Markdown 笔记：打开即编辑，光标所在块显示源码，其余内容保持排版。

*A modeless, live-preview Markdown notebook for the terminal.*

![Platform](https://img.shields.io/badge/platform-macOS-lightgrey)
![Rust](https://img.shields.io/badge/Rust-edition%202024-orange?logo=rust)
![Ratatui](https://img.shields.io/badge/TUI-Ratatui-blue)
![License](https://img.shields.io/badge/license-MIT-green)

</div>

## 简介

LinkNote 是一个用 Rust + Ratatui 编写的本地 Markdown 笔记 TUI。左侧递归浏览文件夹和 Markdown 文件，右侧直接编辑：光标所在的标题、段落、列表、表格或代码块显示原始 Markdown，离开后恢复排版。输入后自动保存，没有编辑模式，也不需要手动保存。

设计理念：

- **文件夹 + Markdown 就是数据源**：不引入专有格式或数据库，笔记仍是普通文件，可以继续用 Git、同步工具和其他编辑器处理。应用状态写在系统应用数据目录，不进入笔记目录。
- **无模式**：没有"查看 / 编辑"之分，打开即可输入。
- **块级源码 / 排版切换**：所有编辑都作用于同一份源码，渲染不会重写 Markdown；跨块选区、粘贴和撤销保持连续。
- **不丢数据**：原子写入，不静默覆盖外部修改，保存失败时不丢弃内存内容，异常退出后可从恢复草稿找回。

## 核心特性

- **混合排版**：标题、分级列表、任务列表、引用、代码块、对齐表格（带边框，按终端显示宽度对齐）、链接与脚注等常见 Markdown 语法。
- **Unicode 友好**：中文、emoji、组合字符按字素簇移动和删除；中文按标点和文字类型分词移动。
- **链接跳转**：`[文字](其他笔记.md#标题)`、`[[维基链接]]`、`#标题` 和网址，F12 打开，Shift+F12 返回；目标笔记不存在时可直接新建。
- **查找**：Ctrl+P 按文件名查找，Tab 切换全文搜索；文内查找与替换使用智能大小写。
- **文件管理**：左栏单字母完成新建、重命名、移动、复制路径和移到系统废纸篓（删除前二次确认）。
- **图片**：本地与 HTTP(S) 图片在正文内显示，自动探测 Kitty / Sixel / iTerm2 图像协议，不支持时退回彩色字符图；网络图片按笔记许可后才加载。
- **Mermaid**：使用锁定的官方 Mermaid 11.17.2 + ZenUML 在本机渲染，示例覆盖 23 类图表。
- **全屏查看器**：F10 放大图片或 Mermaid，支持缩放、平移和交给系统应用打开。
- **可配置**：设置面板（主题、图片、Mermaid、快捷键）与 `keys.json` 自定义快捷键；鼠标点击、拖选、拖动分隔线调整栏宽均可用。

## 安装

需要 Rust 工具链（建议当前 stable；项目使用 edition 2024）。

```sh
git clone https://github.com/Helios5018/linknote.git
cd linknote

# 方式一：构建后直接运行
cargo build --release --locked
./target/release/linknote open ./docs/demo

# 方式二：安装到 Cargo bin 目录，并创建默认笔记目录 ~/Note
sh scripts/install.sh
```

**可选：Mermaid 渲染**。需要 Node.js 22.12+，在项目根目录执行一次：

```sh
npm ci --prefix scripts/mermaid
```

这一步会下载 JavaScript 依赖和 Puppeteer 所需的浏览器；之后渲染不联网、不启动服务。未安装时 Mermaid 图块只显示提示，其余功能不受影响。二进制默认从编译时的项目目录查找该组件，搬动二进制后可用 `LINKNOTE_MERMAID_DIR` 指向 `scripts/mermaid`。

## 30 秒上手

```sh
linknote                  # 打开 ~/Note（不存在时自动创建）
linknote open             # 打开当前目录
linknote open <目录或 .md> # 打开指定目录或笔记
linknote --help           # 命令行帮助
```

在左栏按 `a` 输入 `想法` 新建 `想法.md`（输入 `资料/` 则创建文件夹），然后直接开始写。常用快捷键：

| 快捷键 | 功能 |
| --- | --- |
| `a` / `r` / `m` / `c` / `d`（左栏） | 新建 / 重命名 / 移动 / 复制绝对路径 / 移到废纸篓 |
| Enter、双击或点击正文 | 进入编辑 |
| Ctrl+W / Esc | 在左栏与编辑器之间切换焦点 |
| Ctrl+B | 收起 / 展开左栏 |
| Ctrl+P | 查找笔记，Tab 切换全文搜索 |
| Ctrl+F / Ctrl+G | 文内查找（Tab 打开替换）/ 下一个匹配 |
| F12 / Shift+F12 | 打开光标处的链接 / 返回 |
| F10 | 全屏查看图片或 Mermaid |
| Ctrl+R | 处理保存冲突或重试保存 |
| Ctrl+, | 打开设置 |
| F1 | 帮助：列出当前生效的全部快捷键 |
| Ctrl+Q | 保存后退出 |

撤销、复制粘贴、全选等编辑键同时接受 Ctrl 和 macOS 的 Command（需终端把按键传给应用）。完整快捷键表见 [使用手册](docs/usage.md)。

## 配置要点

配置文件位于应用配置目录（macOS 为 `~/Library/Application Support/linknote/`，其他平台为系统应用目录）：

- `settings.json`：设置面板（Ctrl+,）写入的外观、图片与 Mermaid 选项，改动即时生效。
- `keys.json`：自定义快捷键。为某个动作写出的按键替换默认值，空列表表示取消绑定；配置有误时状态栏提示，其余动作照常使用默认值。

```json
{
  "keys": {
    "search_notes": ["ctrl+o"],
    "back": ["shift+f12", "alt+left"]
  }
}
```

笔记目录中可放置 `.linknoteignore`（每行一个名称，支持 `*` / `?` 通配），左栏和搜索都会忽略匹配项。

常用环境变量：

| 变量 | 作用 |
| --- | --- |
| `LINKNOTE_CONFIG_DIR` | 指定 `settings.json` 与 `keys.json` 所在目录 |
| `LINKNOTE_STATE_DIR` | 指定会话记录与恢复草稿目录 |
| `LINKNOTE_IMAGE_PROTOCOL` | 强制图像协议：`kitty` / `sixel` / `iterm2` / `halfblocks` / `off` |
| `LINKNOTE_REMOTE_IMAGES` | 设为 `1` 时自动加载所有网络图片 |
| `LINKNOTE_TRASH_DIR` | 删除时移到此目录，而不是系统废纸篓 |
| `LINKNOTE_OPENER` | 打开网址和非 Markdown 文件的程序（默认 `open` / `xdg-open`） |
| `LINKNOTE_MERMAID_DIR` | Mermaid 渲染组件目录 |
| `LINKNOTE_CLIPBOARD` | 设为 `internal` 时只使用应用内剪贴板 |

## 数据安全

- 停止输入 500ms 后自动保存，持续输入时最长每 2s 保存一次；切换文件和正常退出也会写回。
- 写入时先在目标目录写临时文件并刷新到磁盘，再原子替换，保留原文件权限；只读文件拒绝覆盖，未编辑的文件不重写。
- 保存前检查磁盘版本：检测到外部修改或删除时停止覆盖，可查看逐行差异，选择另存为 `.local-N.md`、改用磁盘版本（本地内容留在撤销历史）或明确覆盖。
- 尚未写回的编辑约每 150ms 写入恢复草稿，异常退出后重新打开会询问是否恢复。
- 同一数据目录下的多个实例对同一笔记加排他锁；删除默认移到系统废纸篓，不做永久删除。
- 不主动联网：网络图片需按笔记许可，外部链接只在用户触发时交给系统程序打开。

详细规则与边界见 [使用手册 · 保存与恢复](docs/usage.md#保存与恢复)。

## 文档

| 文档 | 内容 |
| --- | --- |
| [docs/usage.md](docs/usage.md) | 完整使用手册：界面操作、全部快捷键、设置、保存与恢复、图片、Mermaid、当前边界 |
| [docs/design.md](docs/design.md) | 设计选择、实验记录、模块划分与后续方向 |
| [docs/demo/](docs/demo/欢迎.md) | 可编辑的示例笔记，含 Markdown 语法与 Mermaid 图表演示 |

## 平台支持与边界

- 已在 macOS（arm64）实测；使用跨平台组件，但尚未在 Linux / Windows 做运行验证。
- 单文件上限 4 MiB，仅支持 UTF-8；CRLF 文件写回仍为 CRLF。
- 终端等宽排版，不能改变标题字号；emoji 宽度与输入法候选窗由终端决定。
- 暂不支持数学公式排版、反向链接列表和同步服务；SVG、动画图片暂不显示。
- 不是多端协同编辑系统：不遵循锁的外部程序与原子替换之间仍有极短的竞争窗口。

## 开发与验证

```sh
npm ci --prefix scripts/mermaid      # Mermaid 组件（Node.js 22.12+）
npm test --prefix scripts/mermaid
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build
python3 scripts/smoke.py             # 真实 PTY 端到端测试
```

验证发布二进制：

```sh
cargo build --release --locked
LINKNOTE_BIN="$PWD/target/release/linknote" python3 scripts/smoke.py
```

测试全部使用临时目录，不会操作个人笔记。

## 技术栈与致谢

- 终端界面与事件：[Ratatui](https://github.com/ratatui/ratatui)、[Crossterm](https://github.com/crossterm-rs/crossterm)
- Markdown 解析：[pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark)
- 文本处理：unicode-segmentation、unicode-width
- 图片：[ratatui-image](https://github.com/ratatui/ratatui-image)、image、ureq
- 图表：[Mermaid](https://github.com/mermaid-js/mermaid)、ZenUML、Puppeteer
- 其他：tempfile、Serde、directories、arboard、percent-encoding、similar（冲突差异）、trash（系统废纸篓）

文档核心维护单一源码、撤销和光标，混合布局维护终端单元格到源码的映射。设计时参考了若干 Markdown 编辑与笔记应用的交互思路，未复制其代码，详见 [docs/design.md](docs/design.md#参考)。

## 许可证

[MIT](LICENSE)。依赖各自保留其许可证。
