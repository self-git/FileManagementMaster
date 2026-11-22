#!/usr/bin/env bash
set -euo pipefail

project=$(cd "$(dirname "$0")/.." && pwd -P)
output="$project/exports"
if [[ -L "$output" ]]; then
  printf '请使用项目内的实际输出目录：%s\n' "$output" >&2
  exit 1
fi

mkdir -p "$output"
scratch=$(mktemp -d "$output/.source-XXXXXX")
trap 'rm -rf -- "$scratch"' EXIT
staging="$scratch/FileManagementMaster"

copy_file() {
  local relative=$1
  local source="$project/$relative"
  local parent
  if [[ ! -f "$source" || -L "$source" ]]; then
    printf '找不到可打包的普通文件：%s\n' "$relative" >&2
    exit 1
  fi
  parent=$(cd "$(dirname "$source")" && pwd -P)
  if [[ "$parent/" != "$project/"* ]]; then
    printf '文件位置超出项目目录：%s\n' "$relative" >&2
    exit 1
  fi
  mkdir -p "$(dirname "$staging/$relative")"
  cp "$source" "$staging/$relative"
}

# 固定入口文件包含锁文件、工作流、Tauri 配置和发版说明。
required_files=(
  .gitignore
  .github/workflows/desktop.yml
  index.html package.json package-lock.json tsconfig.json vite.config.ts
  README.md '需求说明.md' '技术方案与开发计划.md' 'GitHub 发布与安装包上架指南.md'
  scripts/git-runtime.json scripts/prepare-git.mjs scripts/pack-source.sh
  src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/build.rs
  src-tauri/tauri.conf.json src-tauri/tauri.macos.conf.json src-tauri/tauri.windows.conf.json
  src-tauri/capabilities/default.json
  src-tauri/icons/icon.png src-tauri/icons/icon.icns src-tauri/icons/icon.ico
)
for relative in "${required_files[@]}"; do
  copy_file "$relative"
done
if [[ -f "$project/LICENSE" ]]; then
  copy_file LICENSE
fi

# 仅遍历应用源码目录，常见本地目录和隐藏内容直接跳过。
for directory in src src-tauri/src src-tauri/tests; do
  if [[ ! -d "$project/$directory" || -L "$project/$directory" ]]; then
    printf '找不到可打包的源码目录：%s\n' "$directory" >&2
    exit 1
  fi
  find "$project/$directory" \
    \( -name '.*' -o -name node_modules -o -name dist -o -name target \) -prune \
    -o -type f -print0 > "$scratch/sources"
  while IFS= read -r -d '' source; do
    relative=${source#"$project/"}
    case "$relative" in
      src/*.vue|src/*.ts|src/*.css|src/*.js|src/*.json|src/*.svg|src/*.png|src/*.jpg|src/*.webp|src/*.woff2)
        copy_file "$relative"
        ;;
      src-tauri/src/*.rs|src-tauri/tests/*.rs)
        copy_file "$relative"
        ;;
    esac
  done < "$scratch/sources"
done

archive="FileManagementMaster-source-$(date +%Y%m%d-%H%M%S)-${scratch##*-}.zip"
(
  cd "$scratch"
  # 使用 UTF-8 文件名，保证 Python 等工具也能解出中文文档。
  LC_ALL=en_US.UTF-8 /usr/bin/tar --format zip --no-mac-metadata -cf "$archive" FileManagementMaster
)
unzip -tq "$scratch/$archive" >/dev/null
mv "$scratch/$archive" "$output/$archive"
printf '源码包已生成：%s\n' "$output/$archive"
