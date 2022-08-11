# OuiSync CLI

This directory contains code for the command line application interface (CLI)
to the OuiSync library. It can be used to create (`--create`), accept
(`--accept`) or simply open one or multiple repositories.

## Examples:

### Create a repository

To create a repository one would use the following commands

```bash
mkdir -p ~/ouisync/{data,config,mnt} # Make sure the directories exist.
ouisync --create foo --password foo:123 --mount foo:$HOME/ouisync/mnt --share foo:read --data-dir ~/ouisync/data --config-dir ~/ouisync/config
```

Where:

* `--create foo` gives the created repository the name "foo".
* `--password foo:123` sets the password for repository "foo" to be "123".
* `--mount foo:$HOME/ouisync/mnt` (optional) the user will be able to access
  the data at `~/ouisync/mnt` (note the use of the `$HOME` env variable because
  shell won't expand the `~` in this case).
* `--share foo:write` (optional) `ouisync` shall print the "share token" which
  can be sent to other users who wish to sync with this newly created repository.
* `--data-dir ~/ouisync/data` The encrypted repository data shall be stored in `~/ouisync/data/` directory.
* `--config-dir ~/ouisync/config` Additional configuration files shall be stored at `~/ouisync/config/` directory.

### Accept a repository

Once a repository is created, it's share token can be sent to others who wish to keep synchronizing with this repository.
To do so, those users would "accept" the token using the following commands

```bash
mkdir -p ~/ouisync/{data,config,mnt} # Make sure the directories exist.
ouisync --accept <TOKEN> --password foo:234 --mount foo:$HOME/ouisync/mnt --data-dir ~/ouisync/data --config-dir ~/ouisync/config
```

Where:

* `--accept <TOKEN>` instructs `ouisync` to "accept" the token created in the
  previous section. Note that the token contains the repository name which is
  then used in the rest of the flags (the name in the token can be modified
  manually).

The rest of the flags is analogous to those in the previous section.

### Open a repository

When a repository has been created using `--create` or `--accept` flags, it can
later be re-opened (and re-mounted) by issuing

```bash
ouisync --mount foo:$HOME/ouisync/mnt --password foo:XYZ --data-dir ~/ouisync/data --config-dir ~/ouisync/config
```

## Full options list

```console
Command line options

USAGE:
    ouisync [OPTIONS]

OPTIONS:
        --accept <TOKEN>              Accept a share token. Can be specified multiple times to
                                      accept multiple tokens
        --accept-file <PATH>          Accept share tokens by reading them from a file, one token per
                                      line
        --bind <proto/ip:port>        Addresses to bind to. The expected format is
                                      {tcp,quic}/IP:PORT. Note that there may be at most one of each
                                      (protoco x IP-version) combinations. If more are specified,
                                      only the first one is used [default: quic/0.0.0.0:0
                                      quic/[::]:0]
    -c, --create <NAME>               Create new repository with the specified name. Can be
                                      specified multiple times to create multiple repositories
        --config-dir <PATH>           Path to the config directory. Use the --print-dirs flag to see
                                      the default
    -d, --disable-local-discovery     Disable local discovery
        --data-dir <PATH>             Path to the data directory. Use the --print-dirs flag to see
                                      the default
        --disable-dht                 Disable DHT
        --disable-merger              Disable Merger (experimental, will likely be removed in the
                                      future)
        --disable-upnp                Disable UPnP
    -h, --help                        Print help information
        --key <NAME:KEY>              Pre-hashed 32 byte long (64 hexadecimal characters) master
                                      secret per repository. This is mainly intended for testing as
                                      password derivation is rather slow and some of the tests may
                                      timeout if the `password` argument is used instead. For all
                                      other use cases, prefer to use the `password` argument instead
    -m, --mount <NAME:PATH>           Mount the named repository at the specified path. Can be
                                      specified multiple times to mount multiple repositories
        --password <NAME:PASSWD>      Password per repository
        --peers <PEERS>               Explicit list of {tcp,quic}/IP:PORT addresses of peers to
                                      connect to
        --print-device-id             Prints the device id to the stdout when the replica becomes
                                      ready. Note this flag is unstable and experimental
        --print-dirs                  Prints the path to the data and config directories and exits
        --print-port                  Prints the listening port to the stdout when the replica
                                      becomes ready. Note this flag is unstable and experimental
        --share <NAME:ACCESS_MODE>    Print share token for the named repository with the specified
                                      access mode ("blind", "read" or "write"). Can be specified
                                      multiple times to share multiple repositories
        --share-file <PATH>           Print the share tokens to a file instead of standard output
                                      (one token per line)
```

# Building

Install dependencies

    $ sudo apt install pkg-config libfuse-dev

Install Rust using instructions from [rust-lang.org](https://www.rust-lang.org/tools/install).

Buld the app

    $ cargo build --release --bin ouisync

