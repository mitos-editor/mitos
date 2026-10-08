#!/bin/sh
case "$1" in
  -s) echo "$FIXTURE_OS" ;;
  -m) echo "$FIXTURE_ARCH" ;;
esac
