use crate::{
    basic::sync::SpinRwLockWriteGuard,
    memory::uaccess::copy_one_to_user,
    multitask::{cur_task, task::TaskUserIds},
    syscall::SyscallResult,
};

use crate::syscall::err_codes::*;

// 它其实不安全，但只在内部使用所以也没什么所谓
fn cur_uids() -> TaskUserIds {
    *cur_task().user_ids.read()
}

fn cur_uids_write<'a>() -> SpinRwLockWriteGuard<'a, TaskUserIds> {
    cur_task().user_ids.write()
}

pub extern "C" fn getuid() -> usize {
    cur_uids().uid as _
}

pub extern "C" fn geteuid() -> usize {
    cur_uids().euid as _
}

pub extern "C" fn getgid() -> usize {
    cur_uids().gid as _
}

pub extern "C" fn getegid() -> usize {
    cur_uids().egid as _
}

pub extern "C" fn getresuid(ruid: *mut u32, euid: *mut u32, suid: *mut u32) -> SyscallResult {
    let uids = cur_uids();

    copy_one_to_user(ruid, &uids.uid)?;
    copy_one_to_user(euid, &uids.euid)?;
    copy_one_to_user(suid, &uids.suid)?;

    SyscallResult::success(0)
}

pub extern "C" fn getresgid(rgid: *mut u32, egid: *mut u32, sgid: *mut u32) -> SyscallResult {
    let uids = cur_uids();

    copy_one_to_user(rgid, &uids.gid)?;
    copy_one_to_user(egid, &uids.egid)?;
    copy_one_to_user(sgid, &uids.sgid)?;

    SyscallResult::success(0)
}

macro_rules! set_common {
    ($($var:expr),+ => $wanna:expr; if $unprivileged:expr, in $($new:expr),+) => {{
        let tmp = $wanna as i32; // 必须转换，因为 glibc 有时会用零扩展寄存器
        if tmp != -1 {
            let tmp_u32 = tmp as u32;
            if $unprivileged && $(tmp_u32 != $new)&&+ {
                return EPERM;
            }

            $($var = tmp_u32;)+
        }
    }};
}

pub extern "C" fn setresuid(ruid: isize, euid: isize, suid: isize) -> SyscallResult {
    let mut uids = cur_uids_write();
    let unprivileged = uids.euid != 0;

    set_common!(uids.uid              => ruid; if unprivileged, in uids.uid);
    set_common!(uids.euid, uids.fsuid => euid; if unprivileged, in uids.euid);
    set_common!(uids.suid             => suid; if unprivileged, in uids.suid);

    SyscallResult::success(0)
}

pub extern "C" fn setresgid(rgid: isize, egid: isize, sgid: isize) -> SyscallResult {
    let mut uids = cur_uids_write();
    let unprivileged = uids.euid != 0;

    set_common!(uids.gid              => rgid; if unprivileged, in uids.gid);
    set_common!(uids.egid, uids.fsgid => egid; if unprivileged, in uids.egid);
    set_common!(uids.sgid             => sgid; if unprivileged, in uids.sgid);

    SyscallResult::success(0)
}

pub extern "C" fn setreuid(ruid: isize, euid: isize) -> SyscallResult {
    let mut uids = cur_uids_write();
    let unprivileged = uids.euid != 0;

    let old_uid = uids.uid;

    set_common!(uids.uid              => ruid; if unprivileged, in old_uid, uids.euid);
    set_common!(uids.euid, uids.fsuid => euid; if unprivileged, in old_uid, uids.euid, uids.suid);

    if ruid != -1 || (euid != -1 && euid as u32 != old_uid) {
        uids.suid = uids.euid;
    }

    SyscallResult::success(0)
}

pub extern "C" fn setregid(rgid: isize, egid: isize) -> SyscallResult {
    let mut uids = cur_uids_write();
    let unprivileged = uids.euid != 0;

    let old_gid = uids.gid;

    set_common!(uids.gid              => rgid; if unprivileged, in old_gid, uids.sgid);
    set_common!(uids.egid, uids.fsgid => egid; if unprivileged, in old_gid, uids.sgid);

    if rgid != -1 || (egid != -1 && egid as u32 != old_gid) {
        uids.sgid = uids.egid;
    }

    SyscallResult::success(0)
}

pub extern "C" fn setuid(uid: isize) -> SyscallResult {
    let mut uids = cur_uids_write();
    let unprivileged = uids.euid != 0;

    set_common!(uids.uid, uids.euid, uids.suid, uids.fsuid => uid; if unprivileged, in uids.uid, uids.suid);

    SyscallResult::success(0)
}

pub extern "C" fn setgid(gid: isize) -> SyscallResult {
    let mut uids = cur_uids_write();
    let unprivileged = uids.euid != 0;

    set_common!(uids.gid, uids.egid, uids.sgid, uids.fsgid => gid; if unprivileged, in uids.gid, uids.sgid);

    SyscallResult::success(0)
}
