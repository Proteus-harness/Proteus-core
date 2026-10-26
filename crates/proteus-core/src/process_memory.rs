//! Bound glibc arena retention in multi-process workloads before workers start.
pub(super) fn initialize() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // Keep explicit caller tuning, including the glibc tunables interface.
        if std::env::var_os("MALLOC_ARENA_MAX").is_some()
            || std::env::var("GLIBC_TUNABLES").is_ok_and(|value| {
                value
                    .split(':')
                    .any(|entry| entry.starts_with("glibc.malloc.arena_max="))
            })
        {
            return;
        }
        unsafe extern "C" {
            fn mallopt(parameter: std::ffi::c_int, value: std::ffi::c_int) -> std::ffi::c_int;
        }
        // M_ARENA_MAX from glibc malloc.h. No environment mutation after threads
        // start, no allocator replacement; other libc implementations are untouched.
        const M_ARENA_MAX: std::ffi::c_int = -8;
        // SAFETY: mallopt takes integer options, owns no Rust pointers, and is
        // invoked by main before the application starts worker threads.
        unsafe {
            mallopt(M_ARENA_MAX, 2);
        }
    }
}
