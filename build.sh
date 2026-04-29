#!/bin/bash
cd "$(dirname "$0")"
cargo build --release --features memory-candle -p openfang-cli 2>&1
