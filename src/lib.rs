#![allow(unused_unsafe)]

use anyhow::Context;
use log::{error, info};
use std::fs;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use zygisk_rs::{register_zygisk_module, Api, AppSpecializeArgs, Module, ServerSpecializeArgs};

static IS_TARGET: AtomicBool = AtomicBool::new(false);

// Fallback file logging in case android_logger doesn't work
fn flog(msg: &str) {
    let _ = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open("/data/local/tmp/metadump.log")
        .and_then(|mut f| writeln!(f, "{}", msg));
}

struct MetaDump {
    api: Api,
}

impl Module for MetaDump {
    fn new(api: Api, _env: *mut jni::sys::JNIEnv) -> Self {
        flog("[metadump] Module::new() called");
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Info)
                .with_tag("metadump"),
        );
        info!("metadump: Module::new() initialized");
        flog("[metadump] Module::new() done");
        Self { api }
    }

    fn pre_app_specialize(&mut self, args: &mut AppSpecializeArgs) {
        flog("[metadump] pre_app_specialize() called");
        IS_TARGET.store(false, Ordering::Relaxed);
        info!("metadump: pre_app_specialize() called");

        let inner = || -> anyhow::Result<()> {
            // Step 1: Get module directory
            flog("[metadump]   step 1: get_module_dir");
            let module_dir = self
                .api
                .get_module_dir()
                .context("get_module_dir")?;
            flog(&format!("[metadump]   module_dir fd ok"));

            // Step 2: Open list.txt
            flog("[metadump]   step 2: open list.txt");
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
            flog(&format!("[metadump]   list.txt: {}", list_content.trim()));

            // Step 3: Read package name
            flog("[metadump]   step 3: read nice_name");
            let nice_name_ptr = unsafe { *args.nice_name };
            if nice_name_ptr.is_null() {
                flog("[metadump]   nice_name is NULL, dlclose");
                self.api
                    .set_option(zygisk_rs::ModuleOption::DlcloseModuleLibrary);
                return Ok(());
            }

            let len = unsafe { libc::strlen(nice_name_ptr as *const libc::c_char) };
            let slice = unsafe { std::slice::from_raw_parts(nice_name_ptr as *const u8, len) };
            let package_name = String::from_utf8_lossy(slice);
            flog(&format!("[metadump]   package: {}", package_name));

            // Step 4: Check if target
            flog("[metadump]   step 4: check target");
            let found = list_content
                .lines()
                .any(|item| item.trim() == package_name.trim());
            flog(&format!("[metadump]   is_target: {}", found));

            if !found {
                flog("[metadump]   not target, dlclose");
                self.api
                    .set_option(zygisk_rs::ModuleOption::DlcloseModuleLibrary);
                return Ok(());
            }

            IS_TARGET.store(true, Ordering::Relaxed);
            info!("metadump: targeting {}", package_name);
            flog(&format!("[metadump]   TARGET SET: {}", package_name));
            Ok(())
        };

        if let Err(e) = inner() {
            flog(&format!("[metadump] pre_app_specialize ERROR: {:?}", e));
            error!("pre_app_specialize error: {:?}", e);
            self.api
                .set_option(zygisk_rs::ModuleOption::DlcloseModuleLibrary);
        }
        flog("[metadump] pre_app_specialize() done");
    }

    fn post_app_specialize(&mut self, _args: &AppSpecializeArgs) {
        flog("[metadump] post_app_specialize() called");
        let is_target = IS_TARGET.load(Ordering::Relaxed);
        flog(&format!("[metadump]   IS_TARGET: {}", is_target));

        if !is_target {
            flog("[metadump]   skipping (not target)");
            info!("metadump: skipping (not target)");
            return;
        }

        flog("[metadump]   spawning dump thread");
        std::thread::spawn(|| {
            flog("[metadump] dump thread started");
            for i in 0..60 {
                std::thread::sleep(std::time::Duration::from_millis(500));
                flog(&format!("[metadump] poll attempt {}", i));
                match dump_dmabuf_metadata() {
                    Ok(total) => {
                        flog(&format!("[metadump] DUMPED {} bytes", total));
                        info!("metadump: DUMPED {} bytes", total);
                        return;
                    }
                    Err(e) => {
                        flog(&format!("[metadump] poll {} failed: {:?}", i, e));
                        if i % 10 == 0 {
                            info!("metadump: waiting... (attempt {})", i);
                        }
                    }
                }
            }
            flog("[metadump] TIMEOUT - no dmabuf found");
            error!("metadump: timeout waiting for dmabuf:METADATA");
        });
        flog("[metadump] post_app_specialize() done");
    }

    fn pre_server_specialize(&mut self, _args: &mut ServerSpecializeArgs) {
        flog("[metadump] pre_server_specialize() called");
    }
    fn post_server_specialize(&mut self, _args: &ServerSpecializeArgs) {
        flog("[metadump] post_server_specialize() called");
    }
}

register_zygisk_module!(MetaDump);

fn dump_dmabuf_metadata() -> anyhow::Result<usize> {
    flog("[metadump] dump: reading /proc/self/maps");
    let maps = fs::read_to_string("/proc/self/maps")?;
    let mut total: usize = 0;
    let mut found = false;

    flog("[metadump] dump: opening output file");
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

        flog(&format!(
            "[metadump] dump: region {:x}-{:x} ({})",
            start, end, size
        ));
        let data = unsafe { std::slice::from_raw_parts(start as *const u8, size) };
        out.write_all(data)?;
        total += size;
        info!("metadump: dumped {} bytes from {:x}-{:x}", size, start, end);
    }

    out.flush()?;
    flog(&format!("[metadump] dump: total {} bytes", total));

    if !found {
        return Err(anyhow::anyhow!("no dmabuf:METADATA regions found"));
    }

    fs::write(
        "/data/local/tmp/metadata_dump.done",
        format!("dumped {} bytes\n", total),
    )?;

    Ok(total)
}
