//! Process management syscalls
use crate::{
    mm::translated_byte_buffer,
    task::{
        change_program_brk,
        current_user_token,
        exit_current_and_run_next,
        suspend_current_and_run_next,
        get_current_syscall_count,
    },
    timer::get_time_us,
};
use core::mem::size_of;
use crate::mm::{PageTable, PTEFlags, VirtAddr};
use crate::config::PAGE_SIZE;
use crate::mm::MapPermission;
use crate::task::mmap_current;
use crate::task::munmap_current;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// YOUR JOB: get time with second and microsecond
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
pub fn sys_get_time(_ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let us =get_time_us();

    let time = TimeVal {
        sec:us /1_000_000,
        usec:us % 1_000_000,
    };


    let src = unsafe{
        core::slice::from_raw_parts(
            &time as *const TimeVal as *const u8,
            size_of::<TimeVal>(),
        )
    };

    let buffers=translated_byte_buffer(current_user_token(),_ts as *const u8,size_of::<TimeVal>());

    let mut offset =0;

    for buffer in buffers{
        let len =buffer.len();
        buffer.copy_from_slice(&src[offset..offset+len]);
        offset+=len;
    }

    0
}

/// TODO: Finish sys_trace to pass testcases
/// HINT: You might reimplement it with virtual memory management.
pub fn sys_trace(_trace_request: usize, _id: usize, _data: usize) -> isize {
    trace!("kernel: sys_trace");

    match _trace_request{
        0 | 1 => {
            let upper = _id >> 39;
            let sign = (_id >> 38) & 1;
            let expected_upper = if sign == 0 {
                0
            } else {
                usize::MAX >> 39
            };

            if upper != expected_upper {
                return -1;
            }

            let page_table = PageTable::from_token(current_user_token());
            let va=VirtAddr::from(_id);

            let pte = match page_table.translate(va.floor()){
                Some(pte) => pte,
                None => return -1,
            };

            if !pte.is_valid() || !pte.flags().contains(PTEFlags::U){
                return -1;
            }

            if _trace_request == 0 {
                if !pte.readable(){
                    return -1;
                }

                let bytes=pte.ppn().get_bytes_array();
                bytes[va.page_offset()] as isize
            }else {
                if !pte.writable() || !pte.readable(){
                    return -1;
                }

                let bytes=pte.ppn().get_bytes_array();
                bytes[va.page_offset()]=_data as u8;
                0
            }
        }
        2 => get_current_syscall_count(_id) as isize,

        _ => -1,
    }
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
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
