#!/usr/bin/env bash
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

echo "=== [1/5] Pull latest code ==="
git pull

echo "=== [2/5] Build release binary (feishu feature) ==="
cargo build --release --features feishu --bin tiny-claw-feishu

echo "=== [3/5] Restart systemd service ==="
systemctl restart tiny-claw-feishu
sleep 1

echo "=== [4/5] Service status ==="
systemctl status tiny-claw-feishu --no-pager -l

echo "=== [5/5] Recent logs ==="
journalctl -u tiny-claw-feishu -n 20 --no-pager

echo ""
echo "=== Deploy complete ==="
