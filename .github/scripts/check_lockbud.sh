#!/bin/bash

# Run lockbud to check for deadlocks and capture the output
output=$(cargo lockbud -k deadlock -b -l tokio_util)

# Check if lockbud returned any issues
if [[ -n "$output" ]]; then
  # Print the JSON payload
  echo "Lockbud detected issues:"
  echo "$output"
  
  # Exit with a non-zero status to indicate an error
  exit 1
else
  echo "No issues detected by Lockbud."
  exit 0
fi