use anyhow::Context;
use jni::JNIEnv;
use log::{error, info};
use std::fs;
use std::io::{Read, Write};
use zygisk_rs::{register_zygisk_module, Api, AppSpecializeArgs, Module, ServerSpecializeArgs};

struct MetaDump {
    api: Api,
    env: JNIEnv<'static>,
}

impl Module for MetaDump {
    fn new(api: Api, env: *mut jni::sys::JNIEnv) -> Self {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Info)
                .with_tag("metadump"),
        );
        let env = unsafe { JNIEnv::from_raw(env).unwrap() };
        Self { api, env }
    }

    fn pre_app_specialize(&mut self, args: &mut AppSpecializeArgs) {
        let inner = || -> anyhow::Result<()> {
            let package_name = self
                .env
                .get_string(&unsafe { jni::objects::JString::from_raw(*args.nice_name) })?
                .to_string_lossy()
                .to_string();
            info!("pre_app_specialize: {}", package_name);

            let module_dir = self
                .api
                .get_module_dir()
                .context("get_module_dir")?;
            let mut list_file =
                fs::File::open(format!("{}/list.txt", module_dir.display()))?;
            let mut list_content = String::new();
            list_file.read_to_string(&mut list_content)?;

            let found = list_content
                .lines()
                .any(|item| item.trim() == package_name);

            if !found {
                self.api
                    .set_option(zygisk_rs::ModuleOption::DlcloseModuleLibrary);
                return Ok(());
            }

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
        // This runs INSIDE the child process after fork!
        // Spawn a thread to wait for metadata to load, then dump dmabuf
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

    // Write confirmation marker
    fs::write(
        "/data/local/tmp/metadata_dump.done",
        format!("dumped {} bytes\n", total),
    )?;

    Ok(total)
}
