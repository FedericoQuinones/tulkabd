#!/usr/bin/env bash
# Installs the system libraries the Linux build needs.
# A mirror that stops answering makes apt-get wait for hours: each attempt
# is cut after a few minutes and tried again, from Ubuntu's own mirrors
# rather than the runner's. Mirrors are reached over HTTPS: plain HTTP from
# the runners has been seen to stall while HTTPS answered.
set -euo pipefail

packages=(libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev libssl-dev libdbus-1-dev pkg-config)
apt=(sudo apt-get -o Acquire::Retries=3 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30)
# GitHub's Ubuntu runners read their mirrors from this list.
mirrors=/etc/apt/apt-mirrors.txt
if [[ -f $mirrors ]]; then
  sudo sed -i 's#^http://#https://#' "$mirrors"
fi

for attempt in 1 2 3; do
  if timeout 180 "${apt[@]}" update && timeout 480 "${apt[@]}" install -y "${packages[@]}"; then
    exit 0
  fi
  echo "apt-get attempt $attempt failed" >&2
  if [[ -f $mirrors ]]; then
    printf 'https://archive.ubuntu.com/ubuntu/\tpriority:1\nhttps://security.ubuntu.com/ubuntu/\tpriority:2\n' | sudo tee "$mirrors" >/dev/null
  fi
  sleep 15
done
exit 1
