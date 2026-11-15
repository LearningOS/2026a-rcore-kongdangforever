//! Process management syscalls
use alloc::sync::Arc;
use core::mem::size_of;

use crate::{
    config::PAGE_SIZE,
    loader::get_app_data_by_name,
    mm::{
        translated_byte_buffer, translated_refmut,
        translated_str, MapPermission,
    },
    task::{
        add_task, current_task, current_user_token,
        exit_current_and_run_next, mmap_current,
        munmap_current, suspend_current_and_run_next,
        TaskControlBlock,
    },
    timer::get_time_us,
};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(exit_code: i32) -> ! {
    trace!("kernel:pid[{}] sys_exit", current_task().unwrap().pid.0);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel:pid[{}] sys_yield", current_task().unwrap().pid.0);
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
    if let Some(data) = get_app_data_by_name(path.as_str()) {
        let task = current_task().unwrap();
        task.exec(data);
        0
    } else {
        -1
    }
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, return -2.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    trace!("kernel::pid[{}] sys_waitpid [{}]", current_task().unwrap().pid.0, pid);
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

/// YOUR JOB: get time with second and microsecond
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
pub fn sys_get_time(_ts: *mut TimeVal, _tz: usize) -> isize {
    trace!(
        "kernel:pid[{}] sys_get_time NOT IMPLEMENTED",
        current_task().unwrap().pid.0
    );
    let us = get_time_us();

    let time = TimeVal {
        sec: us / 1_000_000,
        usec: us % 1_000_000,
    };

    let src = unsafe {
        core::slice::from_raw_parts(
            &time as *const TimeVal as *const u8,
            size_of::<TimeVal>(),
        )
    };

    let buffers = translated_byte_buffer(
        current_user_token(),
        _ts as *const u8,
        size_of::<TimeVal>(),
    );

    let mut offset = 0;
    for buffer in buffers {
        let len = buffer.len();
        buffer.copy_from_slice(&src[offset..offset + len]);
        offset += len;
    }

    0
}

// YOUR JOB: Implement mmap.
pub fn sys_mmap(_start: usize, _len: usize, _port: usize) -> isize {
    trace!("kernel: sys_mmap NOT IMPLEMENTED YET!");
    if _start % PAGE_SIZE != 0 {
        return -1;
    }

    if _port & !0x7 != 0 || _port & 0x7 == 0 {
        return -1;
    }

    if _len == 0 {
        return 0;
    }

    let end = match _start.checked_add(_len) {
        Some(end) => end,
        None => return -1,
    };

    // 检查 SV39 地址的高位符号扩展。
    let is_sv39_address = |addr: usize| {
        let upper = addr >> 39;
        let sign = (addr >> 38) & 1;
        upper == if sign == 0 { 0 } else { usize::MAX >> 39 }
    };

    if !is_sv39_address(_start)
        || !is_sv39_address(end - 1)
        || (_start >> 38) != ((end - 1) >> 38)
    {
        return -1;
    }

    // 将结束地址向上取整到页边界，并检查溢出。
    let aligned_end = match end.checked_add(PAGE_SIZE - 1) {
        Some(value) => value & !(PAGE_SIZE - 1),
        None => return -1,
    };

    let mut permission = MapPermission::U;

    if _port & 0x1 != 0 {
        permission |= MapPermission::R;
    }
    if _port & 0x2 != 0 {
        permission |= MapPermission::W;
    }
    if _port & 0x4 != 0 {
        permission |= MapPermission::X;
    }

    match mmap_current(_start, aligned_end, permission) {
        Ok(()) => 0,
        Err(()) => -1,
    }
}

// YOUR JOB: Implement munmap.
pub fn sys_munmap(_start: usize, _len: usize) -> isize {
    trace!("kernel: sys_munmap NOT IMPLEMENTED YET!");
    if _start % PAGE_SIZE != 0 {
        return -1;
    }

    if _len == 0 {
        return 0;
    }

    let end = match _start.checked_add(_len) {
        Some(end) => end,
        None => return -1,
    };

    // 与 mmap 使用相同的 SV39 地址检查。
    let is_sv39_address = |addr: usize| {
        let upper = addr >> 39;
        let sign = (addr >> 38) & 1;

        upper == if sign == 0 {
            0
        } else {
            usize::MAX >> 39
        }
    };

    if !is_sv39_address(_start)
        || !is_sv39_address(end - 1)
        || (_start >> 38) != ((end - 1) >> 38)
    {
        return -1;
    }

    let aligned_end = match end.checked_add(PAGE_SIZE - 1) {
        Some(value) => value & !(PAGE_SIZE - 1),
        None => return -1,
    };

    match munmap_current(_start, aligned_end) {
        Ok(()) => 0,
        Err(()) => -1,
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

/// YOUR JOB: Implement spawn.
/// HINT: fork + exec =/= spawn
pub fn sys_spawn(_path: *const u8) -> isize {
    trace!(
        "kernel:pid[{}] sys_spawn NOT IMPLEMENTED",
        current_task().unwrap().pid.0
    );
    // 将用户地址空间中的程序名读取为内核字符串。
    let path = translated_str(current_user_token(), _path);

    // 查找目标程序的 ELF 数据。
    let elf_data = match get_app_data_by_name(path.as_str()) {
        Some(data) => data,
        None => return -1,
    };

    let parent = current_task().unwrap();

    // 直接加载目标程序，构造全新的子进程。
    let child = Arc::new(TaskControlBlock::new(elf_data));
    let child_pid = child.pid.0;

    // 子进程通过弱引用记录父进程。
    {
        let mut child_inner = child.inner_exclusive_access();
        child_inner.parent = Some(Arc::downgrade(&parent));
    }

    // 父进程通过强引用持有子进程。
    {
        let mut parent_inner = parent.inner_exclusive_access();
        parent_inner.children.push(Arc::clone(&child));
    }

    // 加入就绪队列，等待调度运行。
    add_task(child);

    child_pid as isize
}

// YOUR JOB: Set task priority.
pub fn sys_set_priority(_prio: isize) -> isize {
    trace!(
        "kernel:pid[{}] sys_set_priority NOT IMPLEMENTED",
        current_task().unwrap().pid.0
    );
    if _prio < 2 {
        return -1;
    }

    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    inner.priority = _prio as usize;

    _prio
}
