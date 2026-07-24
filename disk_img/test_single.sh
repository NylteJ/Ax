#!/bin/sh
cd /sdcd/glibc
echo "=== Single fs-create test ==="
/bin/simple_llm_server 8080 &
SRV=$!
sleep 1
echo "--- agent output ---"
/bin/agent_lite --workspace . --host 127.0.0.1 --port 8080 "Create a file named test_file.txt with content 'Hello OS'" 2>&1
echo "--- end ---"
echo "Check file:"
ls -la test_file.txt 2>&1
cat test_file.txt 2>&1
echo "---"
kill $SRV 2>/dev/null
wait $SRV 2>/dev/null
echo "Done"
