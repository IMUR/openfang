#!/bin/bash
cd /home/prtr/prj/openfang
cargo check --features memory-candle -p openfang-cli 2>&1
