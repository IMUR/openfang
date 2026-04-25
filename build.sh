#!/bin/bash
cd /home/prtr/prj/openfang
cargo build --release --features memory-candle -p openfang-cli 2>&1
