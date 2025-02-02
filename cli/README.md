# Ouisync CLI

The command line version of Ouisync.

## Installation

Ouisync can be built from source or installed as a docker container.

### Build from source (recommended)

1. Install dependencies:
   
    `sudo apt install pkg-config libfuse-dev`

2. Install Rust using instructions from [rust-lang.org](https://www.rust-lang.org/tools/install).
3. `cd path/to/ouisync' and build the app with:
   
    `cargo build --release --bin ouisync`

4. Find the resulting `ouisync` binary in `target/release`.

### Docker

The Ouisync CLI docker image is available on [dockerhub](https://hub.docker.com/r/equalitie/ouisync):

    docker pull equalitie/ouisync:latest

## Usage

Run `./ouisync --help` to see the available commands. Use `./ouisync COMMAND --help` to see detailed
help for the given command. This document contains brief explanation of the most common commands.

NOTE: All settings are persisted across restarts.

### Start

#### Standalone binary

Run

    ./ouisync start

Which runs Ouisync in the foreground. To run it in the background use e.g. systemd or similar.

#### Docker

Run

    docker run --name ouisync -d [OTHER DOCKER OPTIONS...] equalitie/ouisync:latest

then control it by running

    docker exec ouisync ouisync COMMAND [ARGS...]

It's recommended to setup an alias for this, e.g.:

    alias ouisync=docker exec ouisync ouisync

It's recommended to setup docker volumes or bind mounts for these directories:

- `/config`: config files are stored here
- `/store`: repositories are stored here (this can get big, depending on how many repositories there
  are or how much data is in them)
- `/mount`: repositories are mounted here. This is needed only if one actually wants to mount the
  repositories in order to access their content. Not needed when all the repos are blind replicas
  (e.g. when ouisync runs as a cache server).

### Bind

Before Ouisync can start syncing repositories with other peers it needs to be bound to a network
interface and port using the `bind` command. Up to four endpoints can be specified at a time - one
for each combination of supported protocols (TCP and QUIC) and IP versions (IPv4 and IPv6). For
example, to bind to all interfaces and a random port using the QUIC protocol on both IP versions,
run:

    ouisync bind quic/0.0.0.0:0 quic/[::]:0

Note the docker image is preconfigured to initially bind to port 20209 on all interfaces, both
protocols and both IP versions.

### Manage repositories

#### Create

To create a repository, run

    ouisync create --name NAME

This command is also used to import repositories using their share tokens:

    ouisync create --name NAME --share-token TOKEN

#### Share

To obtain the share token of a repository, run

    ouisync share --name NAME

Note the `--name` argument works also with a prefix of the name as long as it's unique.

The share token can be optionally converted to a QR-code using e.g, [qrencode](https://fukuchi.org/works/qrencode/):

    ouisync share --name NAME | qrencode -o qr.png

#### Mount

To access a repository's files, first mount the repository with:

    ouisync mount --name NAME

This mounts the repository NAME to its default mount point (`$OUISYNC_MOUNT_DIR`/NAME). The repository
can then be accessed like a regular directory on the filesystem.

One can also mount all the repositories at once:

    ouisync mount --all

#### List

To list all repositories, run

    ouisync list-repositories

or simply:

    ouisync ls

This is useful for executing bulk commands, e.g,:

    for repo in $(ouisync ls); do
        ouisync quota -n $repo 100MiB
    done

#### Export and import

A Ouisync repository can be exported to a file in order to back it up, transfer it onto a thumbdrive, etc...:

    ouisync export --name NAME path/to/file

To import the repository's backup file back into Ouisync (on another device, for instance), use:

    ouisync import path/to/file

#### Other

Run `ouisync help` to see all available commands.

### Peer Discovery

Ouisync discovers peers for repositories automatically using *Local Discovery*, *Distributed Hash Table*
(DHT) and *Peer Discovery* (PEX). Local discovery is enabled globally:

    ouisync local-discovery on

DHT and PEX need to be enabled per repository:

    ouisync dht --name NAME on
    ouisync pex --name NAME on

Ouisync peers can also be added explicitly in cases where one doesn't want to or can't use any of
the discovery mechanisms:

    ouisync add-peers quic/198.51.100.0:24816

### Cache servers

A cache server is a Ouisync instance that runs on a server exposed to the internet and which has the
cache server remote API enabled. This API is used by other Ouisync instances to create temporary
replicas of their repositories on the server. The purpose of such a server is improving connectivity
and availability.

The cache server API requires TLS, and so a TLS certificate must be provisioned before enabling it. The
certificate and the private key must be placed (or symlinked) to `$OUISYNC_CONFIG_DIR/cert.pem` and
`$OUISYNC_CONFIG_DIR/key.pem` respectively. Then the API needs to be bound to an interface and
port, e.g.:

    ouisync bind-rpc 0.0.0.0:443

When running a cache server, it's highly recommended to enable *storage quota* and *block
expiration* to prevent the server from running out of storage space when lot of users start using
it.

*Storage quota* is a limit on the amount of data a repository can contain. It can be enabled globally with:

    ouisync quota --default 100MiB

or per repository:

    ouisync quota --name NAME 1GiB

**Note**: The global quota is applied only to newly-created repositories, not already existing ones.

*Block expiration* removes blocks (pieces of files in a repository) that haven't been accessed in
 the given time period. When a block is expired and then requested again, the cache server will try
 to restore it by requesting it again from other peers. Same as storage quota, block expiration
 can be enabled globally:

    ouisync block-expiration --default 43200

or per repository:

    ouisync block-expiration --name NAME 86400

The expiration is specified in seconds.

