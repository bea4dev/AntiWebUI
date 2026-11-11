//! Compiled shaders kept on disk, so that the next launch doesn't compile them again: on DX12, FXC
//! takes 80–400 ms a shader. See "シェーダーのキャッシュ" in `docs/design/39-platform-windows.md`.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A `wgpu::PipelineCache` and the file it is saved in.
pub(crate) struct ShaderCache {
    pub cache: wgpu::PipelineCache,
    path: PathBuf,
    /// How long the data was when last read or written; it only grows.
    saved: AtomicUsize,
    /// Held while writing, so that writes don't interleave.
    writing: Mutex<()>,
}

impl ShaderCache {
    /// Whether to ask for `PIPELINE_CACHE`: on DX12 only, where it holds compiled shaders.
    pub fn wanted(adapter: &wgpu::Adapter) -> bool {
        adapter.get_info().backend == wgpu::Backend::Dx12
            && adapter.features().contains(wgpu::Features::PIPELINE_CACHE)
            && std::env::var("ANTIWEBUI_SHADER_CACHE").map_or(true, |v| v != "off")
            && path().is_some()
    }

    /// The cache of this executable, with what an earlier launch saved.
    pub fn load(device: &wgpu::Device) -> Option<ShaderCache> {
        if !device.features().contains(wgpu::Features::PIPELINE_CACHE) {
            return None;
        }
        let path = path()?;
        let data = std::fs::read(&path).ok();
        let create = |data: Option<&[u8]>| {
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            // SAFETY: the data is what `get_data` gave for this backend; wgpu checks its header.
            let cache = unsafe {
                device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor {
                    label: Some("anti-webui shaders"),
                    data,
                    fallback: true,
                })
            };
            pollster::block_on(scope.pop()).is_none().then_some(cache)
        };
        let (cache, saved) = match data.as_deref().and_then(|d| create(Some(d))) {
            Some(cache) => (cache, data.map_or(0, |d| d.len())),
            None => (create(None)?, 0),
        };
        Some(ShaderCache {
            cache,
            path,
            saved: AtomicUsize::new(saved),
            writing: Mutex::new(()),
        })
    }

    /// Writes the cache on another thread if something was compiled since it was last written.
    pub fn save_soon(self: &std::sync::Arc<Self>) {
        let me = self.clone();
        std::thread::spawn(move || me.save());
    }

    fn save(&self) {
        let _writing = self.writing.lock().unwrap_or_else(|e| e.into_inner());
        let Some(data) = self.cache.get_data() else {
            return;
        };
        if data.len() <= self.saved.load(Ordering::Relaxed) {
            return;
        }
        let Some(dir) = self.path.parent() else {
            return;
        };
        // Written next to it and renamed over it, so that another process never reads half.
        let temp = self
            .path
            .with_extension(format!("{}.tmp", std::process::id()));
        let written = std::fs::create_dir_all(dir)
            .and_then(|()| std::fs::write(&temp, &data))
            .and_then(|()| std::fs::rename(&temp, &self.path));
        match written {
            Ok(()) => self.saved.store(data.len(), Ordering::Relaxed),
            Err(e) => {
                let _ = std::fs::remove_file(&temp);
                eprintln!("[anti-webui] saving the shader cache failed: {e}");
            }
        }
    }
}

/// `%LOCALAPPDATA%\AntiWebUI\shaders\<name of the exe>-<hash of its path>.bin`: one file for
/// each executable, so that apps don't write over each other's.
fn path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty())?;
    let exe = std::env::current_exe().ok()?;
    let mut h = std::hash::DefaultHasher::new();
    exe.hash(&mut h);
    let stem = exe.file_stem()?.to_string_lossy();
    Some(
        PathBuf::from(base)
            .join("AntiWebUI")
            .join("shaders")
            .join(format!("{stem}-{:016x}.bin", h.finish())),
    )
}
