[![CI](https://github.com/equalitie/ouisync/actions/workflows/ci.yml/badge.svg)](https://github.com/equalitie/ouisync/actions/workflows/ci.yml)

# Dependencies

OuiSync uses a number of other Rust libraries that are downloaded during the build process. However, to build and use the OuiSync application (as opposed to just the library), one will additionally need to install the [FUSE](https://www.kernel.org/doc/html/latest/filesystems/fuse.html) library and development files.

    $ sudo apt install libfuse-dev
