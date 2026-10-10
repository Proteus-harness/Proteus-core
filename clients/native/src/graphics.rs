use std::path::Path;

/// Restricts GPU driver probing to the NVIDIA driver on single-GPU NVIDIA
/// machines. The renderer enumerates every Vulkan and EGL driver, and the
/// unused Mesa probes alone keep about 15 MB of LLVM/gallium mapped.
///
/// Must run before any thread is spawned.
pub fn prepare_environment() {
    if !Path::new("/proc/driver/nvidia/version").exists() || render_node_count() != 1 {
        return;
    }
    set_default("VK_LOADER_DRIVERS_SELECT", "*nvidia*");
    let vendor = "/usr/share/glvnd/egl_vendor.d/10_nvidia.json";
    if Path::new(vendor).exists() {
        set_default("__EGL_VENDOR_LIBRARY_FILENAMES", vendor);
    }
}

fn render_node_count() -> usize {
    std::fs::read_dir("/dev/dri")
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("renderD"))
                .count()
        })
        .unwrap_or(0)
}

fn set_default(key: &str, value: &str) {
    if std::env::var_os(key).is_none() {
        // SAFETY: called from `main` before the application spawns threads.
        unsafe { std::env::set_var(key, value) };
    }
}
