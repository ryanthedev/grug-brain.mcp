#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
site_dir="$root/sites/grug-brag-doc"
index_file="$site_dir/index.html"
styles_file="$site_dir/styles.css"
script_file="$site_dir/app.js"

[[ -f "$index_file" ]] || { echo "missing $index_file"; exit 1; }
[[ -f "$styles_file" ]] || { echo "missing $styles_file"; exit 1; }
[[ -f "$script_file" ]] || { echo "missing $script_file"; exit 1; }

grep -q "grug-brain gives your agent a place to keep context" "$index_file" \
  || { echo "hero copy missing"; exit 1; }
grep -q "Brains are stores. Git is history." "$index_file" \
  || { echo "brain history section missing"; exit 1; }
grep -q "organizational primitives" "$index_file" \
  || { echo "organization copy missing"; exit 1; }
grep -q '<base href="/grug-brain/">' "$index_file" \
  || { echo "slug base href missing"; exit 1; }

grep -q "prefers-reduced-motion: reduce" "$styles_file" \
  || { echo "reduced motion handling missing"; exit 1; }
grep -q "data-depth" "$index_file" \
  || { echo "parallax layers missing"; exit 1; }
grep -q "requestAnimationFrame" "$script_file" \
  || { echo "parallax animation loop missing"; exit 1; }
