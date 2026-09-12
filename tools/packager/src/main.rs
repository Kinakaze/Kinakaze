//! Publish native images byte-for-byte under their own PE import names.
mod pe;
use kinakaze_v2_bridge::{ModuleSet, generate_import_library, inspect_import_library, native};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct Args {
    dll_dir: PathBuf,
    exe_dir: Option<PathBuf>,
    dist_dir: PathBuf,
    link_dir: Option<PathBuf>,
    development: bool,
}
fn arguments() -> Result<Args> {
    let mut args = std::env::args_os().skip(1);
    let (mut dll_dir, mut exe_dir, mut dist_dir, mut link_dir) = (None, None, None, None);
    let mut development = false;
    while let Some(option) = args.next() {
        if option == "--development" {
            if development {
                return Err("duplicate option".into());
            }
            development = true;
            continue;
        }
        let target = match option.to_str() {
            Some("--dll-dir") => &mut dll_dir,
            Some("--dist-dir") => &mut dist_dir,
            Some("--exe-dir") => &mut exe_dir,
            Some("--link-dir") => &mut link_dir,
            _ => return Err(format!("unknown argument {option:?}; use --dll-dir DIR --dist-dir DIR [--exe-dir DIR] [--link-dir DIR] [--development]").into()),
        };
        if target.is_some() {
            return Err("duplicate option".into());
        }
        *target = Some(PathBuf::from(args.next().ok_or("missing argument value")?));
    }
    Ok(Args {
        dll_dir: dll_dir.ok_or("--dll-dir is required")?,
        exe_dir,
        dist_dir: dist_dir.ok_or("--dist-dir is required")?,
        link_dir,
        development,
    })
}

fn reject_redirect(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(format!("refusing redirected package path {}", path.display()).into());
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    // FILE_ATTRIBUTE_REPARSE_POINT
                    return Err(format!("refusing reparse point {}", path.display()).into());
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn output_directory(parent: &Path, name: &str) -> Result<PathBuf> {
    let path = parent.join(name);
    reject_redirect(&path)?;
    fs::create_dir_all(&path)?;
    let canonical = path.canonicalize()?;
    if canonical.parent() != Some(parent) {
        return Err("package output directory escaped destination".into());
    }
    Ok(canonical)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    reject_redirect(path)?;
    let parent = path.parent().ok_or("output has no parent")?;
    let name = path
        .file_name()
        .ok_or("output has no filename")?
        .to_string_lossy();
    // create_new ensures we never overwrite a preexisting temporary file.
    let mut temporary = None;
    for attempt in 0..100u32 {
        let candidate = parent.join(format!(
            ".{name}.kinakaze-{}-{attempt}.tmp",
            std::process::id()
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let (temporary_path, mut file) = temporary.ok_or("could not create package temporary file")?;
    let result: Result<()> = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        reject_redirect(path)?;
        fs::rename(&temporary_path, path)?;
        Ok(())
    })();
    if result.is_err() {
        // Only this exact create_new-owned file is eligible for cleanup.
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

struct Staging {
    path: PathBuf,
    files: Vec<PathBuf>,
}

impl Staging {
    fn create(parent: &Path) -> Result<Self> {
        for attempt in 0..100 {
            let path = parent.join(format!(".native-stage-{}-{attempt}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        files: Vec::new(),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err("cannot reserve native staging directory".into())
    }

    fn write(&mut self, name: &str, bytes: &[u8]) -> Result<()> {
        let path = self.path.join(name);
        atomic_write(&path, bytes)?;
        self.files.push(path);
        Ok(())
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        for path in &self.files {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_dir(&self.path);
    }
}

fn package(args: Args) -> Result<()> {
    let source = args.dll_dir.canonicalize()?;
    let executable_source = args.exe_dir.as_deref().unwrap_or(&source).canonicalize()?;
    let mut executables = Vec::new();
    for name in ["init.exe", "worker.exe"] {
        let path = executable_source.join(name);
        reject_redirect(&path)?;
        let bytes = native::read(&path)?;
        // The entry executables must start without PATH or a DLL beside them.
        // Native modules are loaded explicitly from rootfs/lib after bootstrap.
        for dependency in pe::imports(&bytes)? {
            if native::is_shared_object(&dependency) || dependency.starts_with("std-") {
                return Err(
                    format!("{name} has a non-system startup dependency: {dependency}").into(),
                );
            }
        }
        executables.push((name, bytes));
    }
    let mut images = BTreeMap::new();
    for entry in fs::read_dir(&source)? {
        let path = entry?.path();
        if path.extension().and_then(|name| name.to_str()) != Some("dll") {
            continue;
        }
        reject_redirect(&path)?;
        let bytes = native::read(&path)?;
        let import_library = path.with_extension("dll.lib");
        if !import_library.is_file() {
            continue;
        }
        reject_redirect(&import_library)?;
        let name = native::import_name(&native::read(&import_library)?)?;
        if !native::is_shared_object(&name) {
            continue;
        }
        native::exports(&bytes)?;
        if Path::new(&name).file_name().and_then(|name| name.to_str()) != Some(name.as_str()) {
            return Err("native import name must be a filename".into());
        }
        if images.insert(name.clone(), bytes).is_some() {
            return Err(format!("duplicate native image {}", name).into());
        }
    }
    if images.is_empty() {
        return Err("no native shared objects in build directory".into());
    }
    let mut external = BTreeSet::new();
    for bytes in images.values() {
        for name in pe::imports(bytes)? {
            if images.contains_key(&name) {
                continue;
            }
            if native::is_shared_object(&name) {
                return Err(format!("missing native dependency {name}").into());
            }
            if source.join(&name).is_file() {
                external.insert(name);
            }
        }
    }
    for name in external {
        reject_redirect(&source.join(&name))?;
        images.insert(name.clone(), native::read(&source.join(name))?);
    }
    reject_redirect(&args.dist_dir)?;
    fs::create_dir_all(&args.dist_dir)?;
    let dist = args.dist_dir.canonicalize()?;
    if let Some(directory) = &args.link_dir {
        fs::create_dir_all(directory)?;
        if directory.canonicalize()?.starts_with(&dist) {
            return Err("build-only ELF link inputs must stay outside the distribution".into());
        }
    }
    let mut staging = Staging::create(&dist)?;
    // Native DLL initializers require their complete dependency set. No image
    // code, imports, heap entry or standard library byte is changed here.
    for (name, bytes) in &images {
        staging.write(name, bytes)?;
    }
    let modules = ModuleSet::discover(&staging.path)?;
    let pairs: Vec<_> = modules
        .modules
        .iter()
        .map(|module| (module, images[&module.soname].as_slice()))
        .collect();
    let dependencies: Vec<_> = images
        .iter()
        .filter(|(name, _)| !modules.modules.iter().any(|m| &m.soname == *name))
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    pe::validate_package_with_dependencies(&pairs, &dependencies)?;
    let mut imports = Vec::new();
    for module in &modules.modules {
        if args.link_dir.is_some() || args.development {
            let bytes = generate_import_library(module)?;
            inspect_import_library(&bytes, module)?;
            imports.push((module.soname.clone(), bytes));
        }
        println!(
            "{} ({} native exports)",
            module.soname,
            module.exports.len()
        );
    }
    let count = modules.modules.len();
    drop(modules);
    let root = output_directory(&dist, "rootfs")?;
    let documentation = output_directory(
        &output_directory(
            &output_directory(&output_directory(&root, "usr")?, "share")?,
            "doc",
        )?,
        "kinakaze-libc",
    )?;
    atomic_write(
        &documentation.join("copyright"),
        include_bytes!("../../../libs/libc/third-party-notices.txt"),
    )?;
    let libraries = output_directory(&root, "lib")?;
    for (name, bytes) in &images {
        atomic_write(&libraries.join(name), bytes)?;
    }
    for (name, bytes) in &executables {
        atomic_write(&dist.join(name), bytes)?;
    }
    if let Some(directory) = &args.link_dir {
        for (name, bytes) in &imports {
            atomic_write(&directory.join(name), bytes)?;
        }
    }
    if args.development {
        // Linux compilers consume ELF symbol/version tables, not Windows PE.
        // The unversioned development files carry the native runtime SONAME;
        // DT_NEEDED therefore still selects rootfs/lib's real PE implementation.
        let usr = output_directory(&root, "usr")?;
        let lib = output_directory(&usr, "lib")?;
        let directory = output_directory(&lib, "x86_64-linux-gnu")?;
        for (name, bytes) in &imports {
            if name.starts_with("lib")
                && let Some((stem, _)) = name.split_once(".so")
            {
                atomic_write(&directory.join(format!("{stem}.so")), bytes)?;
            }
        }
    }
    println!(
        "Published init.exe, worker.exe and rootfs ({count} native modules) to {}",
        dist.display()
    );
    Ok(())
}
fn main() {
    if let Err(error) = arguments().and_then(package) {
        eprintln!("packager: {error}");
        std::process::exit(1);
    }
}
