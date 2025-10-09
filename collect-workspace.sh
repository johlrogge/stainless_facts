#!/usr/bin/env bash
# Recursively find and output all relevant source and configuration files

# Start with root Cargo.toml and Nix files
find . -maxdepth 1 -type f \( -name "*.nix" -o -name "Cargo.toml" \) \
    | sort \
    | while read -r file; do
        echo "---- ${file#./}"
        cat "$file"
        echo
done

# Find and output all component and base source files
find . -mindepth 2 -type f \( -name "*.rs" -o -name "Cargo.toml" \) \
    -not -path "./target/*" \
    -not -path "./.git/*" \
    | sort \
    | while read -r file; do
        echo "---- ${file#./}"
        cat "$file"
        echo
done