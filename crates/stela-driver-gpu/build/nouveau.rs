//! nouveau: the `nouveau-y` composite of its own Kbuild, compiled unchanged in
//! Stela's Linux environment, and the kernel services it imports that the
//! environment does not yet supply.
use super::kbuild::{Config, Kbuild};
use super::native::Linux;
use super::port::{self, Port};
use std::{fs, path::Path};
use stela_c_build::linux::Module;
use stela_c_build::linux_capture::UnitSpec;

const NOUVEAU: &str = "../../third_party/linux/drivers/gpu/drm/nouveau";
const AUTOCONF: &str = "../stela-linux/src/compat/generated/autoconf.h";
const MODULE: Module<'static> = Module {
    name: "nouveau",
    file: "drivers/gpu/drm/nouveau/nouveau",
};
/// Kbuild's composite: nouveau's DRM layer over NVIF and NVKM.
const COMPOSITES: [&str; 1] = ["nouveau"];

/// The kernel services the composites import that Stela's Linux environment
/// lacks, or defines only as a trap, by service. Building fails when the
/// imports change, so each supplied service leaves this list with its change.
const OPEN: &[(&str, &[&str])] = &[
    // drm_cache.c's x86 code (src/compat/linux/kernel/drm_cache.c): the
    // fallbacks for a processor without CLFLUSH and the SSE4.1 copy, which
    // cpufeature.h never reports.
    (
        "x86 cache fallbacks",
        &["kernel_fpu_begin", "kernel_fpu_end", "wbinvd_on_all_cpus"],
    ),
    // Traps nothing nouveau runs reaches: TTM allocates from its own pool
    // and Stela never swaps it out.
    (
        "swap and page cache",
        &[
            "folio_clear_dirty_for_io",
            "mm_get_unmapped_area",
            "shmem_writeout",
        ],
    ),
    // Nothing tears a pseudo filesystem's superblock down.
    (
        "reopening a file without a VFS path",
        &["stela_linux_reopen_unowned_path"],
    ),
    (
        "HDMI audio",
        &["device_link_add", "platform_device_unregister"],
    ),
];

/// The unit of nouveau's own `module_init`.
const INITCALLS: UnitSpec = UnitSpec {
    scope: stela_c_build::linux_capture::Scope {
        slice: "gpu",
        group: "nouveau",
        group_ordinal: 8,
        unit_ordinal: 1,
        global_ordinal: 8,
    },
    source: "drivers/gpu/drm/nouveau/nouveau_drm",
    module: MODULE.name,
    file: MODULE.file,
};

pub fn port(linux: &mut Linux) {
    let config = Config::parse(&fs::read_to_string(AUTOCONF).expect("read Stela's Kconfig header"))
        .unwrap_or_else(|error| panic!("{AUTOCONF}: {error}"));
    let kbuild =
        Kbuild::read(Path::new(NOUVEAU), &config).unwrap_or_else(|error| panic!("{error}"));
    for file in &kbuild.files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    let mut sources = Vec::new();
    for composite in COMPOSITES {
        let objects = kbuild
            .objects(composite)
            .unwrap_or_else(|error| panic!("{error}"));
        sources.extend(
            objects
                .iter()
                .map(|object| Path::new(NOUVEAU).join(object).with_extension("c")),
        );
    }
    let includes = kbuild
        .includes
        .iter()
        .map(|directory| {
            let relative = directory
                .strip_prefix(NOUVEAU)
                .expect("Kbuild includes lie in nouveau's tree");
            Path::new("nouveau").join(relative).display().to_string()
        })
        .collect::<Vec<_>>();
    let includes = includes.iter().map(String::as_str).collect::<Vec<_>>();
    port::build(
        linux,
        &Port {
            module: MODULE,
            includes: &includes,
            defines: &[],
            initcalls: INITCALLS,
            image: "stela_linux_nouveau_image",
            archive: "stela-nouveau-nvkm",
            open: OPEN,
        },
        &sources,
    );
}
