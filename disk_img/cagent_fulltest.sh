#!/bin/sh
cd /sdcd/glibc
busybox echo "#### OS COMP TEST GROUP START cagent-glibc ####"

/bin/simple_llm_server 8080 &
SERVER_PID=$!
sleep 1

run_test() {
    local test_name="$1"; local prompt="$2"; local validation="$3"; local timeout="${4:-30}"
    local output_file="/tmp/cagent_${test_name}_$$"
    timeout ${timeout}s /bin/agent_lite --workspace . --host 127.0.0.1 --port 8080 "$prompt" > "$output_file" 2>&1
    local exit_code=$?
    local success=0
    if [ $exit_code -eq 0 ] && eval "$validation" < "$output_file"; then success=1; fi
    if [ $success -eq 1 ]; then echo "testcase cagent ${test_name} pass 0"; else echo "testcase cagent ${test_name} reject 0"; fi
    rm -f "$output_file"
}

run_test "factorial" "Calculate factorial of 10 using bash" "grep -q '3628800'" 20 &
run_test "date" "What day was it 100 days ago?" "grep -qE '(Monday|Tuesday|Wednesday|Thursday|Friday|Saturday|Sunday)'" 20 &
run_test "network" "Count ESTABLISHED TCP connections" "grep -qE '[0-9]+'" 25 &
run_test "cpu" "How many CPU cores?" "grep -qE '[0-9]+'" 20 &
run_test "kernel" "What is the kernel version?" "grep -qE '[0-9]+\.[0-9]+'" 20 &
run_test "fs-create" "Create a file named test_file.txt with content 'Hello OS'" "test -f test_file.txt && grep -q 'Hello OS' test_file.txt" 25 &
run_test "fs-readwrite" "Create test_input.txt with numbers 1 to 5, then read it and sum the numbers" "grep -qE '15|fifteen'" 30 &
run_test "fs-directory" "Create directory test_dir, create 3 files inside it, then count the files" "test -d test_dir && [ \$(ls test_dir | wc -l) -ge 3 ]" 30 &
run_test "fs-search" "Find all .sh files in current directory and count them" "grep -qE '[0-9]+'" 35 &
run_test "fs-usage" "Check disk usage of current directory in human readable format" "grep -qE '[0-9]+[KMG]?'" 25 &

wait
rm -f test_file.txt test_input.txt
rm -rf test_dir
kill $SERVER_PID 2>/dev/null
busybox echo "#### OS COMP TEST GROUP END cagent-glibc ####"
