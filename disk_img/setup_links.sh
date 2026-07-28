#!/bin/sh
# Create busybox applet symlinks in /bin
B=/sdcd/musl/busybox
cd /bin
$B ln -sf $B busybox 2>&1
$B echo LN1:$?
$B ln -sf $B sh 2>&1
$B echo LN2:$?
$B ln -sf $B rm 2>&1
$B echo LN3:$?
$B ln -sf $B ls 2>&1; $B ln -sf $B cat 2>&1; $B ln -sf $B cp 2>&1; $B ln -sf $B mv 2>&1; $B ln -sf $B mkdir 2>&1; $B ln -sf $B touch 2>&1; $B ln -sf $B chmod 2>&1; $B ln -sf $B echo 2>&1; $B ln -sf $B grep 2>&1; $B ln -sf $B sed 2>&1; $B ln -sf $B awk 2>&1; $B ln -sf $B wc 2>&1; $B ln -sf $B date 2>&1; $B ln -sf $B timeout 2>&1; $B ln -sf $B find 2>&1; $B ln -sf $B df 2>&1; $B ln -sf $B stat 2>&1; $B ln -sf $B ss 2>&1; $B ln -sf $B nproc 2>&1; $B ln -sf $B uname 2>&1; $B ln -sf $B uptime 2>&1; $B ln -sf $B whoami 2>&1; $B ln -sf $B kill 2>&1; $B ln -sf $B sleep 2>&1; $B ln -sf $B head 2>&1; $B ln -sf $B test 2>&1; $B ln -sf $B printf 2>&1
$B echo LINKS_DONE
