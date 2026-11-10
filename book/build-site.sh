#!/bin/sh

set -eu

book_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
site_dir="$book_dir/site"

rm -rf "$site_dir"
mkdir -p "$site_dir"
mdbook build "$book_dir" --dest-dir "$site_dir/.build"
mv "$site_dir/.build/html" "$site_dir/docs"
rm -rf "$site_dir/.build"

cp "$book_dir/homepage/index.html" \
  "$book_dir/homepage/landing.css" \
  "$book_dir/homepage/404.html" \
  "$book_dir/brand.css" \
  "$book_dir/theme/favicon.svg" \
  "$book_dir/theme/favicon.png" \
  "$book_dir/install/install.sh" \
  "$book_dir/install/install.ps1" \
  "$book_dir/install/_headers" \
  "$site_dir/"
cp -R "$book_dir/install/checksums" "$site_dir/checksums"
