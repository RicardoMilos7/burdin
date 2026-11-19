#!/usr/bin/env bash
# Runs a command, retrying with a growing pause when it fails: retry.sh <command> [args...]
#
# The Ubuntu mirrors behind GitHub-hosted runners sometimes answer 503 for minutes at a time. apt's own
# Acquire::Retries covers one bad download; this covers a failed `apt-get update`, `apt-get install`, or
# `llvm.sh` as a whole. RETRY_ATTEMPTS (default 5) and RETRY_PAUSE seconds (default 20, times the attempt number)
# tune it. The last failure's exit status is the script's.

attempts=${RETRY_ATTEMPTS:-5}
pause=${RETRY_PAUSE:-20}
[ "$#" -gt 0 ] || { echo "usage: retry.sh <command> [args...]" >&2; exit 2; }

attempt=1
while true; do
    "$@" && exit 0
    status=$?
    if [ "$attempt" -ge "$attempts" ]; then
        echo "retry.sh: '$*' failed $attempts times (last status $status)" >&2
        exit "$status"
    fi
    wait=$((pause * attempt))
    echo "retry.sh: '$*' failed with status $status (attempt $attempt of $attempts); retrying in ${wait}s" >&2
    sleep "$wait"
    attempt=$((attempt + 1))
done
