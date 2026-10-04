#!/bin/sh
set -eu

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cargo install --path "$project_root" --locked "$@"
mkdir -p "$HOME/Note"
printf '%s\n' 'LinkNote 已安装，默认笔记目录：' "$HOME/Note" \
    "可在 shell 配置中添加：alias ln='linknote'" \
    "                      alias lno='linknote open'"
