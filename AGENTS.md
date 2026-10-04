# LinkNote

本地 Markdown 笔记 TUI，Rust + Ratatui。启动时打开一个目录，左侧递归浏览文件夹与 Markdown，右侧无模式编辑，当前 Markdown 块显示源码，其他块保持排版，自动保存。

## 重要约束
- 面向用户默认简体中文。文件夹与 Markdown 是数据源，不引入专有笔记格式。
- 所有编辑操作作用于同一份源码，渲染不能重写 Markdown；光标使用 UTF-8 字节边界，移动和删除按字素簇。
- 不得静默覆盖外部修改，不得在保存失败时丢弃内存内容。没有手动保存步骤（Ctrl+S 只是立即保存）。
- 不主动发起网络请求：网络图片需用户按笔记许可；外部链接只在用户触发时交给系统程序。
- 不启用 pulldown-cmark 的 `ENABLE_WIKILINKS`（0.13.4 会在部分输入上 panic），维基链接在 `src/layout/markdown.rs` 中识别。
- 正式代码在 `src/`，验证在 `tests/`；临时实验在 `scratch/`，完成后清理无用产物。
- 不启动后台服务；终端交互实测用本项目 temporary tmux session 并在结束后清理。

## 验证
Mermaid 渲染与完整测试需要先 `npm ci --prefix scripts/mermaid`（Node.js 22.12+）；渲染组件改动还需 `npm test --prefix scripts/mermaid`。

`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`。
编辑、布局、存储修改需要相应行为测试；应用交互用 `tests/app.rs` 直接驱动 `App`；排版缓存改动需跑 `PROPTEST_CASES=100000 cargo test --release --test layout_cache`；界面变动还需运行 `python3 scripts/smoke.py` 做真实 PTY 验证。

## 阅读入口
- 产品交互、实现步骤与实验结论：`docs/design.md`
- 项目首页与安装：`README.md`；使用方法与快捷键：`docs/usage.md`
