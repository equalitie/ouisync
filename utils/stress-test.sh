#!/bin/bash

set -eEu

self_pid=$$

function date_tag {
    date --utc +"%Y-%m-%dT%H:%M:%S"
}

function descendant_pids() {
    pids=$(pgrep -P $1)
    echo $pids
    for pid in $pids; do
        descendant_pids $pid
    done
}

temp_dir_prefix="ouisync-stress-test"
build_args="--release"

concurrency=1
timeout=
log_open=""
log_dump=""
package="ouisync"
test=""

while getopts "dohp:t:T:n:" arg; do
    case $arg in
        p)
            package=$OPTARG
            ;;
        t)
            test=$OPTARG
            ;;
        T)
            timeout=$OPTARG
            ;;
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
            echo "  -p PACKAGE   Package to build (default: $package)"
            echo "  -t TEST      If specified, runs integration test TEST, otherwise runs unit tests"
            echo "  -T TIMEOUT   Timeout for a single invocation of a test process (default: no timeout, example: 10s)"
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

if [ -n "$package" ]; then
    build_args="$build_args --package $package"
fi

if [ -n "$test" ]; then
    build_args="$build_args --test $test"
else
    build_args="$build_args --lib"
fi

# Create the work dir
rm -rf "/tmp/$temp_dir_prefix-*"

dir=`mktemp --tmpdir -d $temp_dir_prefix-XXXXXX`

function cleanup() {
    # Kill all descendants (not just children) of this script
    for pid in $(descendant_pids $self_pid); do
        kill $pid 2>/dev/null || true
    done

    # Remove the work dir
    rm -rf "$dir"
}

trap cleanup INT TERM EXIT ERR

# Override TMPDIR so that all temp files/directories created in the tests are created inside the
# work dir for easier cleanup.
export TMPDIR=$dir
export TEMP=$dir # for windows

echo "$(date_tag) Compiling the test with '$build_args'"
cargo test $build_args --no-run

# This next one will not compile again, we need it to get the executable name.
exe=$(cargo test $build_args --no-run 2>&1 | grep "Executable" | sed "s/^.*Executable.*(\(.*\)).*$/\1/")

if [ -n "$timeout" ]; then
    exe="timeout $timeout $exe"
fi

pipe="$dir/pipe"
mkfifo $pipe

echo "$(date_tag) Working in directory $dir"
echo "$(date_tag) Starting $concurrency tests ('$exe $args')"

export RUST_BACKTRACE=full
export PROPTEST_CASES=32

for process in $(seq $concurrency); do
    {
        # Bind the pipe to the file descriptor 3 so we can echo to it without constantly reopening
        # it.
        exec 3> $pipe

        local_iteration=0

        while true; do
            if $exe $args > $dir/test-$process.log 2>&1; then
                ((local_iteration=local_iteration+1))
                echo "$process $local_iteration ok" >&3
            else
                status=$?
                echo "$(date_tag) Process $process aborted with status $status after $local_iteration iterations"
                echo "$process $local_iteration fail" >&3
                break
            fi
        done

        # Close the file descriptor 3
        exec 3>&-
    } &
done

echo "$(date_tag) Awaiting first process to fail"

global_iteration=0
aborted_process=""

while true; do
    if read -r process local_iteration status; then
        if [ "$status" = "ok" ]; then
            ((global_iteration=global_iteration+1))
            echo "$(date_tag) Iteration #$global_iteration ($process/$local_iteration)"
        else
            echo "$(date_tag) FAIL ($process/$local_iteration)"
            aborted_process=$process
            break;
        fi
    else
        echo "$(date_tag) Failed to read from pipe"
    fi
done < $pipe

new_log_name="/tmp/ouisync-log-$(date_tag).txt"
mv $dir/test-$aborted_process.log $new_log_name
echo "$(date_tag) Log saved to $new_log_name"

if [ -n "$log_open" -a -n "${EDITOR-}" ]; then
    $EDITOR $new_log_name
fi

if [ -n "$log_dump" ]; then
    cat $new_log_name
fi

