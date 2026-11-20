//! The host calls wasi-libc makes on behalf of Luau's C++, answered inside the
//! module so the page supplies no imports beyond wasm-bindgen's: wasi-libc
//! reaches each through an `__imported_wasi_snapshot_preview1_*` symbol, which a
//! definition here satisfies before the linker would import it. No environment,
//! no files, a clock at zero, nothing to exit to.

const EBADF: i32 = 8;

#[unsafe(no_mangle)]
unsafe extern "C" fn __imported_wasi_snapshot_preview1_environ_sizes_get(
    count: *mut u32,
    size: *mut u32,
) -> i32 {
    unsafe {
        *count = 0;
        *size = 0;
    }
    0
}

#[unsafe(no_mangle)]
extern "C" fn __imported_wasi_snapshot_preview1_environ_get(_: i32, _: i32) -> i32 {
    0
}

#[unsafe(no_mangle)]
unsafe extern "C" fn __imported_wasi_snapshot_preview1_clock_time_get(
    _: i32,
    _: i64,
    out: *mut u64,
) -> i32 {
    unsafe { *out = 0 };
    0
}

#[unsafe(no_mangle)]
extern "C" fn __imported_wasi_snapshot_preview1_fd_close(_: i32) -> i32 {
    EBADF
}

#[unsafe(no_mangle)]
extern "C" fn __imported_wasi_snapshot_preview1_fd_fdstat_get(_: i32, _: i32) -> i32 {
    EBADF
}

#[unsafe(no_mangle)]
extern "C" fn __imported_wasi_snapshot_preview1_fd_seek(_: i32, _: i64, _: i32, _: i32) -> i32 {
    EBADF
}

#[unsafe(no_mangle)]
unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_write(
    _: i32,
    _: i32,
    _: i32,
    written: *mut u32,
) -> i32 {
    unsafe { *written = 0 };
    EBADF
}

#[unsafe(no_mangle)]
extern "C" fn __imported_wasi_snapshot_preview1_proc_exit(_: i32) -> ! {
    core::arch::wasm32::unreachable()
}
