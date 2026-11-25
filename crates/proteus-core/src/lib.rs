pub(crate) use proteus_contracts::{contracts, domain, model_standard};

pub mod app_server;
pub mod core;
#[cfg(feature = "memory-diagnostics")]
pub(crate) mod memory_diagnostics;
pub mod process_adapters;
pub(crate) mod stubs;
pub(crate) mod tools;

pub(crate) fn release_unused_allocator_pages() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> std::ffi::c_int;
        }
        // SAFETY: this thread-safe glibc hint takes no pointers and releases
        // only unused pages without moving live allocations.
        unsafe { malloc_trim(0) };
    }
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
extern crate self as proteus_core;
#[cfg(test)]
#[path = "../tests/support/model.rs"]
pub(crate) mod test_model;
