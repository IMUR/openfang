#!/bin/bash
cd "$(dirname "$0")"
cargo check --features memory-candle -p openfang-cli 2>&1
