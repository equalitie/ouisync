A crate for capturing log output and writing it to a file

This works similar to the `tee` command where the log messages are still sent to their original
destination but additonally they are written to the specified file. The file is also rotated.

On linux and windows, this just captures the stdout. On android it invokes `logcat` and pipes its
output to the file. On ios and macos the implementation is still TODO, but the idea is to use
`OsLogStore`.
