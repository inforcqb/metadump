use anyhow::Context;
use log::{error, info};
use std::fs;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use zygisk_rs::{register_zygisk_module, Api, AppSpecializeArgs, Module, ServerSpecializeArgs};

static IS_TARGET: AtomicBool = AtomicBool::new(false);

struct MetaDump {
    api: Api,
}

impl Module for MetaDump {
    fn new(api: Api, _env: *mut jni::sys::JNIEnv) -> Self {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Info)
                .with_tag("metadump"),
        );
        Self { api }
    }

    fn pre_app_specialize(&mut self, args: &mut AppSpecializeArgs) {
        IS_TARGET.store(false, Ordering::Relaxed);

        let inner = || -> anyhow::Result<()> {
            // Only check list.txt to decide if we target this app.
            // Avoid JNI in system processes to prevent crashes.
            let module_dir = self
                .api
                .get_module_dir()
                .context("get_module_dir")?;

            let mut list_file = unsafe {
                fs::File::from_raw_fd(
                    nix::fcntl::openat(
                        Some(module_dir.as_raw_fd()),
                        "list.txt",
                        nix::fcntl::OFlag::O_CLOEXEC,
                        nix::sys::stat::Mode::empty(),
                    )?,
                )
            };
            let mut list_content = String::new();
            list_file.read_to_string(&mut list_content)?;

            // Read package name from nice_name (raw pointer)
            let nice_name_ptr = unsafe { *args.nice_name };
            if nice_name_ptr.is_null() {
                self.api
                    .set_option(zygisk_rs::ModuleOption::DlcloseModuleLibrary);
                return Ok(());
            }

            // Safe: just check if the pointer contains our target string
            let len = unsafe { libc::strlen(nice_name_ptr as *const libc::c_char) };
            let slice = unsafe { std::slice::from_raw_parts(nice_name_ptr as *const u8, len) };
            let package_name = String::from_utf8_lossy(slice);

            let found = list_content
                .lines()
                .any(|item| item.trim() == package_name.trim());

            if !found {
                self.api
                    .set_option(zygisk_rs::ModuleOption::DlcloseModuleLibrary);
                return Ok(());
            }

            IS_TARGET.store(true, Ordering::Relaxed);
            info!("metadump: targeting {}", package_name);
            Ok(())
        };

        if let Err(e) = inner() {
            error!("pre_app_specialize error: {:?}", e);
            self.api
                .set_option(zygisk_rs::ModuleOption::DlcloseModuleLibrary);
        }
    }

    fn post_app_specialize(&mut self, _args: &AppSpecializeArgs) {
        // ONLY run in the target app process
        if !IS_TARGET.load(Ordering::Relaxed) {
            return;
        }

        // Spawn a thread to wait for metadata to load
        std::thread::spawn(|| {
            for i in 0..60 {
                std::thread::sleep(std::time::Duration::from_millis(500));
                match dump_dmabuf_metadata() {
                    Ok(total) => {
                        info!("metadump: DUMPED {} bytes", total);
                        return;
                    }
                    Err(_) => {
                        if i % 10 == 0 {
                            info!("metadump: waiting... (attempt {})", i);
                        }
                    }
                }
            }
            error!("metadump: timeout waiting for dmabuf:METADATA");
        });
    }

    fn pre_server_specialize(&mut self, _args: &mut ServerSpecializeArgs) {}
    fn post_server_specialize(&mut self, _args: &ServerSpecializeArgs) {}
}

register_zygisk_module!(MetaDump);

fn dump_dmabuf_metadata() -> anyhow::Result<usize> {
    let maps = fs::read_to_string("/proc/self/maps")?;
    let mut total: usize = 0;
    let mut found = false;

    let out = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open("/data/local/tmp/metadata_dump.bin")?;

    let mut out = std::io::BufWriter::new(out);

    for line in maps.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 6 {
            continue;
        }
        let path = parts.get(5).copied().unwrap_or("");
        if !path.contains("dmabuf:METADATA") {
            continue;
        }

        found = true;
        let addrs: Vec<&str> = parts[0].split('-').collect();
        if addrs.len() != 2 {
            continue;
        }
        let start = usize::from_str_radix(addrs[0], 16)?;
        let end = usize::from_str_radix(addrs[1], 16)?;
        let size = end - start;

        let data = unsafe { std::slice::from_raw_parts(start as *const u8, size) };
        out.write_all(data)?;
        total += size;
        info!("metadump: dumped {} bytes from {:x}-{:x}", size, start, end);
    }

    out.flush()?;

    if !found {
        return Err(anyhow::anyhow!("no dmabuf:METADATA regions found"));
    }

    fs::write(
        "/data/local/tmp/metadata_dump.done",
        format!("dumped {} bytes\n", total),
    )?;

    Ok(total)
}
