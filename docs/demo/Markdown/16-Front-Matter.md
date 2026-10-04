---
title: Front Matter 测试
tags: [markdown, yaml]
date: 2026-10-02
draft: false
---

# Front Matter

预期：LinkNote 未启用元数据块，文件开头的 YAML 会按 CommonMark 解析——第一行 `---` 是分隔线，最后一行 `---` 让上面几行变成 Setext 二级标题。若将来支持 Front Matter，这块应显示为元数据或保持源码，正文从本标题开始。

编辑元数据并保存后，用其他工具（Obsidian、Hugo）打开，元数据应保持不变。
