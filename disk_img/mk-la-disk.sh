#!/bin/bash
set -e
LA_SRC=/tmp/disk_img_la
rm -rf "$LA_SRC"
cp -r ./disk_img "$LA_SRC"
cd "$LA_SRC/bin"
# Replace RISC-V base-name files with symlinks to LoongArch -la versions
rm -f busybox agent_lite simple_llm_server
rm -f sh rm ls cat cp mv mkdir touch chmod echo grep sed awk wc date timeout find df stat ss nproc uname uptime whoami kill sleep head test printf ln
ln -sf busybox-la busybox
ln -sf agent_lite-la agent_lite
ln -sf simple_llm_server-la simple_llm_server
for app in sh rm ls cat cp mv mkdir touch chmod echo grep sed awk wc date timeout find df stat ss nproc uname uptime whoami kill sleep head test printf ln; do
    ln -sf busybox-la "$app"
done
cd "$OLDPWD"
mkfs.ext4 -F -O ^dir_index -d "$LA_SRC" disk-la.img 10M
