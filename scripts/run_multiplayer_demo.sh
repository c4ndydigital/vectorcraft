#!/usr/bin/env bash
set -euo pipefail

# Ensure working directory is project root
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$DIR"

# Ensure Cargo/Rust environment is sourced
if [ -f /root/.cargo/env ]; then
    source /root/.cargo/env
fi

echo "Starting VectorCraft Multiplayer Collaboration Server on 0.0.0.0:8767..."
echo "Accessible via http://192.168.10.19:8767"

exec cargo run --release -p vectorcraft-server -- --bind 0.0.0.0 --port 8767 --dist dist/web "$@"
