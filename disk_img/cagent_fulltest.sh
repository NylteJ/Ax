#!/bin/sh
# Detect architecture: try RISC-V busybox, fall back to LoongArch
if /bin/busybox true 2>/dev/null; then
    BB=/bin/busybox
    AGENT=/bin/agent_lite
    SERVER=/bin/simple_llm_server
else
    BB=/bin/busybox-la
    AGENT=/bin/agent_lite-la
    SERVER=/bin/simple_llm_server-la
fi
export PATH=/bin
# Clean up leftover files from previous runs first
cd /sdcd/glibc
$BB rm -rf test_dir test_file.txt test_input.txt sd_test_file.txt 2>/dev/null
$BB rm -rf /tmp/_cagent_fsdir 2>/dev/null

$BB echo "#### OS COMP TEST GROUP START cagent-glibc ####"

$SERVER 8080 &
SERVER_PID=$!
$BB sleep 1

run_test() {
    local test_name="$1"; local prompt="$2"; local validation="$3"; local timeout="${4:-30}"
    local output_file="/tmp/cagent_${test_name}_$$"
    $BB timeout ${timeout}s $AGENT --workspace . --host 127.0.0.1 --port 8080 "$prompt" > "$output_file" 2>&1
    local exit_code=$?
    local success=0
    if [ $exit_code -eq 0 ] && eval "$validation" < "$output_file"; then success=1; fi
    if [ $success -eq 1 ]; then $BB echo "testcase cagent ${test_name} pass 0"; else $BB echo "testcase cagent ${test_name} reject 0"; fi
    $BB rm -f "$output_file"
}

run_test "factorial" "Calculate factorial of 10 using bash" "$BB grep -q '3628800'" 20 &
run_test "date" "What day was it 100 days ago?" "$BB grep -qE '(Monday|Tuesday|Wednesday|Thursday|Friday|Saturday|Sunday)'" 20 &
run_test "network" "Count ESTABLISHED TCP connections" "$BB grep -qE '[0-9]+'" 25 &
run_test "cpu" "How many CPU cores?" "$BB grep -qE '[0-9]+'" 20 &
run_test "kernel" "What is the kernel version?" "$BB grep -qE '[0-9]+\.[0-9]+'" 20 &
run_test "fs-create" "Create a file named test_file.txt with content 'Hello OS'" "$BB test -f test_file.txt && $BB grep -q 'Hello OS' test_file.txt" 25 &
run_test "fs-readwrite" "Create test_input.txt with numbers 1 to 5, then read it and sum the numbers" "$BB grep -qE '15|fifteen'" 30 &
run_test "fs-directory" "Create directory test_dir, create 3 files inside it, then count the files" "$BB test -d test_dir && [ \$($BB ls test_dir | $BB wc -l) -ge 3 ]" 30 &
run_test "fs-search" "Find all .sh files in current directory and count them" "$BB grep -qE '[0-9]+'" 35 &
run_test "fs-usage" "Check disk usage of current directory in human readable format" "$BB grep -qE '[0-9]+[KMG]?'" 25 &

# Wait for background test jobs (each has a timeout, 35s max + buffer)
$BB sleep 40
$BB rm -f test_file.txt test_input.txt
$BB rm -rf test_dir
$BB echo "#### OS COMP TEST GROUP END cagent-glibc ####"
$BB kill $SERVER_PID 2>/dev/null
