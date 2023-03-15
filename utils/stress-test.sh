#!/bin/bash

set -e

function date_tag {
    date +"%Y-%m-%d--%H-%M-%S"
}

temp_dir_prefix="ouisync-stress-test"

package="-p ouisync"

if [ -z "$1" ]; then
    echo "Use test name as the first argument"
    exit
fi

test="$1"
rm -rf "/tmp/$temp_dir_prefix-*"

echo "$(date_tag) Compiling the test"
cargo test $test $package --no-run 

# This next one will not compile again, we need it to get the executable name.
exe=$(cargo test $test $package --no-run 2>&1 | grep ' Executable unittests src/lib\.rs')
exe=$(echo $exe | cut -d " " -f 4 | cut -d "(" -f 2 | cut -d ")" -f 1)

dir=`mktemp --tmpdir -d $temp_dir_prefix-XXXXXX`
n=100

echo "$(date_tag) Working in directory $dir"

echo "$(date_tag) Starting $n tests"
for i in $(seq $n); do
    while true; do
        RUST_BACKTRACE=full PROPTEST_CASES=32 $exe $test -- $skip $nocapture $threads 2>&1
        r=$?
        if [ "$r" -ne "0" ]; then break; fi
    done > $dir/test-$i.log 2>&1 &

    pids[${i}]=$!
done

aborted_process="";

echo "$(date_tag) Awaiting first process to fail"
while [ -z "$aborted_process" ]; do
    for i in $(seq $n); do
        pid=${pids[${i}]}
        if ! ps $pid > /dev/null; then
            echo "Process $i (pid:$pid) aborted"
            aborted_process=$i
            break;
        fi
    done
    sleep 0.5
done

echo "$(date_tag) Killing rest of the jobs:"

for i in $(seq $n); do
    pid=${pids[${i}]}
    if [ $i -ne $aborted_process ]; then
        echo "  killing job:$i with pid:$pid"
        pkill -P $pid 2>/dev/null 1>&2 & # || true 
        rm $dir/test-$i.log
    fi
done

new_log_name=/tmp/ouisync-log-$(date +"%Y-%m-%d--%H-%M-%S").txt
mv $dir/test-$aborted_process.log $new_log_name
echo "$(date_tag) Log saved to $new_log_name"

if [ -n "$EDITOR" ]; then
    $EDITOR $new_log_name
fi
