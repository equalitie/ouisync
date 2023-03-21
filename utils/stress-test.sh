#!/bin/bash

set -eu

function date_tag {
    date +"%Y-%m-%dT%H:%M:%S"
}

temp_dir_prefix="ouisync-stress-test"
build_args="--release -p ouisync --lib"

concurrency=1
log_open=""
log_dump=""

while getopts "dohn:" arg; do
    case $arg in
        n)
            concurrency=$OPTARG
            ;;
        o)
            log_open=1
            ;;
        d)
            log_dump=1
            ;;
        h|*)
            echo "Usage $(basename $0) [OPTIONS] <ARGS>..."
            echo
            echo "Options:"
            echo "  -n NUMBER    Number of processes to run concurrently (default: $concurrency)"
            echo "  -d           Dump log to stdout"
            echo "  -o           Open log in \$EDITOR"
            echo "  -h           Show this help"
            echo
            echo "Arguments:"
            echo "  <ARGS>...  Arguments passed verbatim to the test executable"

            exit -1
            ;;
    esac
done

# Extract remaining arguments
shift $((OPTIND-1))
args=$@

trap "pkill -P $$" EXIT

rm -rf "/tmp/$temp_dir_prefix-*"

echo "$(date_tag) Compiling the test"
cargo test $build_args --no-run

# This next one will not compile again, we need it to get the executable name.
exe=$(cargo test $build_args --no-run 2>&1 | grep ' Executable unittests src/lib\.rs')
exe=$(echo $exe | cut -d " " -f 4 | cut -d "(" -f 2 | cut -d ")" -f 1)

dir=`mktemp --tmpdir -d $temp_dir_prefix-XXXXXX`

pipe="$dir/pipe"
mkfifo $pipe

echo "$(date_tag) Working in directory $dir"
echo "$(date_tag) Starting $concurrency tests"

export RUST_BACKTRACE=full
export PROPTEST_CASES=32

for process in $(seq $concurrency); do
    {
        local_iteration=0

        while true; do
            if $exe $args > $dir/test-$process.log 2>&1; then
                ((local_iteration=local_iteration+1))
                echo "$process $local_iteration ok" > $pipe
            else
                echo "$process $local_iteration fail" > $pipe
                break
            fi
        done
    } &
done

echo "$(date_tag) Awaiting first process to fail"

global_iteration=0
aborted_process=""

while read process local_iteration status < $pipe; do
    if [ "$status" = "ok" ]; then
        ((global_iteration=global_iteration+1))
        echo "$(date_tag) Iteration #$global_iteration ($process/$local_iteration)"
    else
        aborted_process=$process
        echo "$(date_tag) Process $aborted_process aborted after $local_iteration iterations"
        break;
    fi
done

new_log_name="/tmp/ouisync-log-$(date_tag).txt"
mv $dir/test-$aborted_process.log $new_log_name
echo "$(date_tag) Log saved to $new_log_name"

if [ -n "$log_open" -a -n "$EDITOR" ]; then
    $EDITOR $new_log_name
fi

if [ -n "$log_dump" ]; then
    cat $new_log_name
fi

