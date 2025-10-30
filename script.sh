#!/bin/bash

# Register med custom=true
custom_registers=(61584 61590 61591 61582 61510 61511 60270 60271 60272 60273 60274 60275)

# Alla register
all_registers=(61513 61517 61534 61538 61546 61550 61554 61558 61562 61584 61590 61591 61521 61582 61510 61511 60270 60271 60272 60273 60274 60275)

api_url="http://192.168.10.19:3000/api/v1/ctc"

for addr in "${all_registers[@]}"; do
  if [[ " ${custom_registers[@]} " =~ " ${addr} " ]]; then
    url="${api_url}?addr=$addr&custom=true"
  else
    url="${api_url}?addr=$addr"
  fi

  value=$(curl -s "$url" | grep -oP '"ctc_data":\s*\K[0-9\.-]+')
  echo "Register $addr: $value"
done
