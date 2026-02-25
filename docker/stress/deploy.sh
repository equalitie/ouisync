#!/usr/bin/env bash

# Build and push the image to the given host without publishing it to a docker repository first.

set -euo pipefail

host=${1:-}

if [ -z "$host" -o "$host" = "-h" -o "$host" = "--help" ]; then
    echo "Build and upload the docker image to the given host"
    echo "Usage: $(basename $0) <HOST>"
    exit 1
fi

docker compose build
docker save ouisync/stress | bzip2 | pv | ssh $host docker image load
