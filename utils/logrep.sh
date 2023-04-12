#!/usr/bin/bash

# Filter log records matching a pattern.

file=
pattern=

if [ $# -gt 1 ]; then
    file="$1"
    pattern="$2"
elif [ $# -gt 0 ]; then
    pattern="$1"
else
    echo "Usage"
    echo ""
    echo "    $(basename $0) PATTERN"
    echo "    $(basename $0) FILE PATTERN"

    exit -1
fi

awk "/$pattern/" RS="\n\n" ORS="\n\n" "$file" | grep --color -E "$pattern|$"
