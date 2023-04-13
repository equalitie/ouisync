#!/usr/bin/bash

# Filter log records matching a pattern.

if [ -z "$1" ]; then
    echo "Usage $(basename $0) PATTERN [FILE]"
    exit -1
fi

pattern="$1"
file="$2"

awk "/$pattern/" RS="\n\n" ORS="\n\n" "$file" | grep --color -E "$pattern|$"
