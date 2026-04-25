#!/bin/bash
cd /home/prtr/prj/openfang
cargo check --features memory-candle -p openfang-cli > /tmp/build_out.txt 2>&1
echo "EXIT_CODE=$?" >> /tmp/build_out.txt
