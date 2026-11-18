//! Process management syscalls
//!
use alloc::sync::Arc;
use core::mem::size_of;

use crate::{
    config::PAGE_SIZE,
    fs::{open_file, OpenFlags},
    mm::{translated_byte_buffer, translated_refmut, translated_str, MapPermission},
    task::{
        add_task, current_task, current_user_token, exit_current_and_run_next, mmap_current,
        munmap_current, suspend_current_and_run_next, TaskControlBlock,
    },
    timer::get_time_us,
};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

pub fn sys_exit(exit_code: i32) -> ! {
    trace!("kernel:pid[{}] sys_exit", current_task().unwrap().pid.0);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_yield() -> isize {
    //trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

pub fn sys_getpid() -> isize {
    trace!("kernel: sys_getpid pid:{}", current_task().unwrap().pid.0);
    current_task().unwrap().pid.0 as isize
}

pub fn sys_fork() -> isize {
    trace!("kernel:pid[{}] sys_fork", current_task().unwrap().pid.0);
    let current_task = current_task().unwrap();
    let new_task = current_task.fork();
    let new_pid = new_task.pid.0;
    // modify trap context of new_task, because it returns immediately after switching
    let trap_cx = new_task.inner_exclusive_access().get_trap_cx();
    // we do not have to move to next instruction since we have done it before
    // for child process, fork returns 0
    trap_cx.x[10] = 0;
    // add new task to scheduler
    add_task(new_task);
    new_pid as isize
}

pub fn sys_exec(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_exec", current_task().unwrap().pid.0);
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(app_inode) = open_file(path.as_str(), OpenFlags::RDONLY) {
        let all_data = app_inode.read_all();
        let task = current_task().unwrap();
        task.exec(all_data.as_slice());
        0
    } else {
        -1
    }
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, return -2.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    //trace!("kernel: sys_waitpid");
    let task = current_task().unwrap();
    // find a child process

    // ---- access current PCB exclusively
    let mut inner = task.inner_exclusive_access();
    if !inner
        .children
        .iter()
        .any(|p| pid == -1 || pid as usize == p.getpid())
    {
        return -1;
        // ---- release current PCB
    }
    let pair = inner.children.iter().enumerate().find(|(_, p)| {
        // ++++ temporarily access child PCB exclusively
        p.inner_exclusive_access().is_zombie() && (pid == -1 || pid as usize == p.getpid())
        // ++++ release child PCB
    });
    if let Some((idx, _)) = pair {
        let child = inner.children.remove(idx);
        // confirm that child will be deallocated after being removed from children list
        assert_eq!(Arc::strong_count(&child), 1);
        let found_pid = child.getpid();
        // ++++ temporarily access child PCB exclusively
        let exit_code = child.inner_exclusive_access().exit_code;
        // ++++ release child PCB
        *translated_refmut(inner.memory_set.token(), exit_code_ptr) = exit_code;
        found_pid as isize
    } else {
        -2
    }
    // ---- release current PCB automatically
}

/// Get the current time, including when the output structure crosses a page.
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel:pid[{}] sys_get_time", current_task().unwrap().pid.0);
    let us = get_time_us();
    let time = TimeVal {
        sec: us / 1_000_000,
        usec: us % 1_000_000,
    };
    let src = unsafe {
        core::slice::from_raw_parts(&time as *const TimeVal as *const u8, size_of::<TimeVal>())
    };
    let mut copied = 0;
    for buffer in
        translated_byte_buffer(current_user_token(), ts as *const u8, size_of::<TimeVal>())
    {
        let len = buffer.len();
        buffer.copy_from_slice(&src[copied..copied + len]);
        copied += len;
    }
    0
}

/// Create an anonymous user mapping.
pub fn sys_mmap(start: usize, len: usize, port: usize) -> isize {
    trace!("kernel:pid[{}] sys_mmap", current_task().unwrap().pid.0);
    if start % PAGE_SIZE != 0 || port & !0x7 != 0 || port & 0x7 == 0 {
        return -1;
    }
    if len == 0 {
        return 0;
    }
    let end = match start.checked_add(len) {
        Some(end) => end,
        None => return -1,
    };
    let is_sv39_address = |addr: usize| {
        let upper = addr >> 39;
        let sign = (addr >> 38) & 1;
        upper == if sign == 0 { 0 } else { usize::MAX >> 39 }
    };
    if !is_sv39_address(start) || !is_sv39_address(end - 1) || (start >> 38) != ((end - 1) >> 38) {
        return -1;
    }
    let aligned_end = match end.checked_add(PAGE_SIZE - 1) {
        Some(value) => value & !(PAGE_SIZE - 1),
        None => return -1,
    };
    let mut permission = MapPermission::U;
    if port & 0x1 != 0 {
        permission |= MapPermission::R;
    }
    if port & 0x2 != 0 {
        permission |= MapPermission::W;
    }
    if port & 0x4 != 0 {
        permission |= MapPermission::X;
    }
    if mmap_current(start, aligned_end, permission).is_ok() {
        0
    } else {
        -1
    }
}

/// Remove a complete user mapping range.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_munmap", current_task().unwrap().pid.0);
    if start % PAGE_SIZE != 0 {
        return -1;
    }
    if len == 0 {
        return 0;
    }
    let end = match start.checked_add(len) {
        Some(end) => end,
        None => return -1,
    };
    let is_sv39_address = |addr: usize| {
        let upper = addr >> 39;
        let sign = (addr >> 38) & 1;
        upper == if sign == 0 { 0 } else { usize::MAX >> 39 }
    };
    if !is_sv39_address(start) || !is_sv39_address(end - 1) || (start >> 38) != ((end - 1) >> 38) {
        return -1;
    }
    let aligned_end = match end.checked_add(PAGE_SIZE - 1) {
        Some(value) => value & !(PAGE_SIZE - 1),
        None => return -1,
    };
    if munmap_current(start, aligned_end).is_ok() {
        0
    } else {
        -1
    }
}

/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel:pid[{}] sys_sbrk", current_task().unwrap().pid.0);
    if let Some(old_brk) = current_task().unwrap().change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}

/// Create a child directly from an executable in the filesystem.
pub fn sys_spawn(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_spawn", current_task().unwrap().pid.0);
    let path = translated_str(current_user_token(), path);
    let app_inode = match open_file(path.as_str(), OpenFlags::RDONLY) {
        Some(inode) => inode,
        None => return -1,
    };
    let elf_data = app_inode.read_all();
    let parent = current_task().unwrap();
    let child = Arc::new(TaskControlBlock::new(elf_data.as_slice()));
    let child_pid = child.pid.0;
    {
        child.inner_exclusive_access().parent = Some(Arc::downgrade(&parent));
    }
    {
        parent
            .inner_exclusive_access()
            .children
            .push(Arc::clone(&child));
    }
    add_task(child);
    child_pid as isize
}

/// Set the current task's stride-scheduling priority.
pub fn sys_set_priority(prio: isize) -> isize {
    trace!(
        "kernel:pid[{}] sys_set_priority",
        current_task().unwrap().pid.0
    );
    if prio < 2 {
        return -1;
    }
    current_task().unwrap().inner_exclusive_access().priority = prio as usize;
    prio
}
