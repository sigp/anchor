#!/bin/bash

# TOML Reader - A script to read values from simple TOML files
# Usage: ./toml_reader.sh <file_path> <section> <key>

get_section() {
    # Function to get the section from a TOML file
    # Parameters:
    # $1 - TOML file path
    # $2 - section name
    local file="$1"
    local section="$2"

    sed -n "/^\[$section\]/,/^\[/p" "$file" | sed '$d'
}

get_toml_value() {
    # Function to get a value from a TOML file
    # Parameters:
    # $1 - TOML file path
    # $2 - section
    # $3 - key
    local file="$1"
    local section="$2"
    local key="$3"

    get_section "$file" "$section" | grep "^$key " | cut -d "=" -f2- | tr -d ' "'
}

# Show usage if script is called directly
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    if [ "$#" -ne 3 ]; then
        echo "Error: Incorrect number of arguments"
        echo "Usage: $0 <file_path> <section> <key>"
        echo "Example: $0 ./config.toml server_b domain"
        exit 1
    fi

    # Check if file exists
    if [ ! -f "$1" ]; then
        echo "Error: File '$1' does not exist"
        exit 1
    fi

    # Get the value
    result=$(get_toml_value "$1" "$2" "$3")

    # Check if value was found
    if [ -z "$result" ]; then
        echo "Error: No value found for section '[$2]' and key '$3'"
        exit 1
    fi

    echo "$result"
fi
