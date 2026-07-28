#!/bin/sh
export PATH=/bin

echo "===== DATE DEBUG ====="
echo "Test 1: date +%s"
date +%s
echo "Test 2: busybox date +%s"
busybox date +%s
echo "Test 3: date -d @0"
date -d @0 '+%A, %B %d, %Y' 2>&1
echo "Test 4: busybox date -d @0"
busybox date -d @0 '+%A, %B %d, %Y' 2>&1
echo "Test 5: arithmetic"
echo $((100 - 50))
echo "Test 6: full date command"
date -d @$(($(date +%s) - 8640000)) '+%A, %B %d, %Y' 2>&1
echo "Test 7: via sh -c"
/bin/busybox sh -c 'date -d @$(($(date +%s) - 8640000)) "+%A, %B %d, %Y"' 2>&1
echo "===== COMPLETE ====="
