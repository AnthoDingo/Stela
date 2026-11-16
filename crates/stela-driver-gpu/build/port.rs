//! A DRM driver port: its unchanged sources compiled in Stela's Linux
//! environment with its own `module_init` captured, and the kernel services
//! its archive imports that the environment does not supply, checked against
//! the port's own list.
use super::native::{self, Linux};
use object::{Object, ObjectSymbol, read::archive::ArchiveFile};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    thread,
};
use stela_c_build::linux::Module;
use stela_c_build::linux_capture::{self as initcalls, Projection, Unit, UnitSpec};

const KERNEL: &str = "../stela-linux/src/compat/kernel";
/// What the compiler itself calls, which every platform's runtime supplies.
const RUNTIME: [&str; 4] = ["memcpy", "memmove", "memset", "memcmp"];
/// The prefix of what Stela's Rust side exports to the environment.
const PLATFORM: &str = "stela_linux_";
/// The prefix of the hosted test harness (`stela/test.h`). Only a hosted
/// environment built with its source fixtures reaches it, from fixture code
/// sharing archive members with the services a port imports.
const HARNESS: &str = "stela_test_";

/// What distinguishes one driver port's build.
pub struct Port<'a> {
    pub module: Module<'static>,
    /// Include directories, relative to Linux's `drivers/gpu/drm`.
    pub includes: &'a [&'a str],
    /// Preprocessor definitions the driver's own Kbuild adds.
    pub defines: &'a [&'a str],
    /// The unit of the driver's own `module_init`.
    pub initcalls: UnitSpec,
    /// The registration image's symbol, which the driver's Rust side starts.
    pub image: &'a str,
    pub archive: &'a str,
    /// The kernel services the archive imports that the environment lacks or
    /// defines only as a trap, by service. Building fails when the imports
    /// change, so each supplied service leaves this list with its change.
    pub open: &'a [(&'a str, &'a [&'a str])],
}

/// Compile `sources` as `port` describes, archive them with the captured
/// registrations, and check the archive's imports.
pub fn build(linux: &mut Linux, port: &Port, sources: &[PathBuf]) {
    let unit = Unit::from(port.initcalls);
    // Compiled below, with its registrations captured.
    let sources = sources
        .iter()
        .filter(|source| **source != unit.path())
        .collect::<Vec<_>>();
    let mut build = stela_c_build::native::configure();
    linux.environment.apply(
        &mut build,
        None,
        &native::drm_includes(port.includes),
        Some(&port.module),
    );
    for define in port.defines {
        build.define(define, None);
    }
    stela_c_build::linux_build::upstream_warnings(&mut build);
    // As Kbuild builds every profile: Linux relies on the optimizer to drop
    // the calls its configuration disables.
    build.opt_level(2).cargo_metadata(false);
    let jobs = env::var("NUM_JOBS")
        .ok()
        .and_then(|jobs| jobs.parse().ok())
        .unwrap_or(1)
        .max(1);
    let objects = thread::scope(|scope| {
        sources
            .chunks(sources.len().div_ceil(jobs))
            .map(|chunk| {
                let build = &build;
                scope.spawn(move || build.clone().files(chunk).compile_intermediates())
            })
            .collect::<Vec<_>>()
            .into_iter()
            .flat_map(|compiling| compiling.join().expect("a compiling thread"))
            .collect::<Vec<_>>()
    });
    let mut captured = linux.take_captured();
    let environment_count = captured.len();
    let compiler = initcalls::compiler(&build);
    let placement = initcalls::Placement::new(
        &unit.path(),
        &stela_c_build::linux_build::out_dir(),
        &[
            Path::new("../.."),
            compiler.parent().expect("compiler root"),
        ],
    )
    .unwrap_or_else(|error| panic!("{} source placement: {error:?}", port.module.name));
    captured.push(initcalls::compile(
        &build,
        Path::new("../stela-linux/src/compat/stela/registration.h"),
        port.initcalls,
        &placement,
    ));
    let mut expected = native::environment_units();
    expected.push(port.initcalls);
    let image = initcalls::source(
        &captured,
        Projection {
            symbol: port.image,
            expected: &expected,
            imported_units: environment_count,
            descriptors_only: false,
        },
    );
    build
        .objects(objects)
        .objects(
            captured
                .iter()
                .skip(environment_count)
                .map(|unit| unit.object()),
        )
        .file(image)
        .compile(port.archive);
    let output = stela_c_build::linux_build::out_dir();
    check(
        port.module.name,
        port.open,
        &output.join(format!("lib{}.a", port.archive)),
        &[&linux.archive, &linux.common_archive],
    );
    let digest = initcalls::write(
        &output.join("captured-units.bin"),
        &captured[initcalls::COMMON.len()..],
    );
    println!("cargo:output={}", output.display());
    println!("cargo:cohort_sha256={digest}");
    // Hosted programs test the environment and link no driver.
    if env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "uefi") {
        println!("cargo:rustc-link-lib=static={}", port.archive);
    }
}

/// Compare the services a port's archive reaches when linked, directly or
/// through the environment's objects, and that nothing supplies, with
/// `expected`: the port's `OPEN` list.
pub fn check(name: &str, expected: &[(&str, &[&str])], port: &Path, environment: &[&Path]) {
    let port = members(port);
    let environment = environment
        .iter()
        .flat_map(|archive| members(archive))
        .collect::<Vec<_>>();
    let definers = environment
        .iter()
        .flat_map(|member| member.defined.iter().map(move |symbol| (symbol, member)))
        .collect::<BTreeMap<_, _>>();
    let defined = port
        .iter()
        .flat_map(|member| &member.defined)
        .collect::<BTreeSet<_>>();
    let traps = traps();
    let harness = env::var_os("DEP_STELA_LINUX_HOSTED_FIXTURES").is_some();
    let mut reached = port
        .iter()
        .flat_map(|member| &member.imported)
        .filter(|symbol| !defined.contains(symbol))
        .collect::<Vec<_>>();
    let mut seen = reached.iter().copied().collect::<BTreeSet<_>>();
    let mut open = BTreeSet::new();
    while let Some(symbol) = reached.pop() {
        if traps.contains(symbol) {
            open.insert(symbol.clone());
        }
        match definers.get(symbol) {
            Some(member) => reached.extend(
                member
                    .imported
                    .iter()
                    .filter(|&import| !defined.contains(import) && seen.insert(import)),
            ),
            None if RUNTIME.contains(&symbol.as_str())
                || symbol.starts_with(PLATFORM)
                || (harness && symbol.starts_with(HARNESS)) => {}
            None => {
                open.insert(symbol.clone());
            }
        }
    }
    let mut stale = Vec::new();
    for symbol in expected.iter().flat_map(|(_, symbols)| symbols.iter()) {
        if !open.remove(*symbol) {
            stale.push(*symbol);
        }
    }
    assert!(
        open.is_empty() && stale.is_empty(),
        "{name}'s imports changed: newly open {open:?}; now supplied {stale:?}"
    );
}

/// What one archive member defines and imports.
struct Member {
    defined: BTreeSet<String>,
    imported: BTreeSet<String>,
}

fn members(archive: &Path) -> Vec<Member> {
    let data = fs::read(archive).unwrap_or_else(|error| panic!("{}: {error}", archive.display()));
    let archive = ArchiveFile::parse(&*data).expect("an archive");
    archive
        .members()
        .map(|member| {
            let member = member.expect("an archive member");
            let object = object::File::parse(member.data(&*data).expect("member data"))
                .expect("an object file");
            let (mut defined, mut imported) = (BTreeSet::new(), BTreeSet::new());
            for symbol in object.symbols() {
                let Ok(name) = symbol.name() else {
                    continue;
                };
                // A weak reference links to nothing when nothing defines it,
                // as `symbol_get` without modules relies on (i915's IPS link).
                if symbol.is_undefined() && !symbol.is_weak() {
                    imported.insert(name.to_owned());
                } else if symbol.is_global() {
                    defined.insert(name.to_owned());
                }
            }
            Member { defined, imported }
        })
        .collect()
}

/// What the environment defines without serving: functions that trap
/// (`STELA_UNSUPPORTED`), placeholder objects (`STELA_UNSUPPORTED_OBJECT`)
/// and functions that report themselves unsupported and fail.
fn traps() -> BTreeSet<String> {
    let mut traps = BTreeSet::new();
    for source in stela_c_build::linux_build::sources(KERNEL)
        .into_iter()
        .chain(stela_c_build::linux_build::sources(
            "src/compat/linux/kernel",
        ))
    {
        let text = fs::read_to_string(&source).expect("read a kernel-API source");
        for (marker, field) in [
            ("STELA_UNSUPPORTED(", 1),
            ("STELA_UNSUPPORTED_OBJECT(", 1),
            ("STELA_UNSUPPORTED_ALIAS(", 0),
            ("stela_linux_unsupported(\"", 0),
        ] {
            for definition in text.split(marker).skip(1) {
                let end = definition.find([')', '"']).unwrap_or(definition.len());
                if let Some(name) = definition[..end].split(',').nth(field) {
                    traps.insert(name.trim().to_owned());
                }
            }
        }
    }
    traps
}
