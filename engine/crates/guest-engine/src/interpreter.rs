//! Direct ld.so invocation and dependency inspection, before guest execution.
use kinakaze_link::{ImmutableBytes, LinkError, Linker, SearchPaths};
use std::path::PathBuf;

pub(super) struct Program {
    pub path: PathBuf,
    pub image: ImmutableBytes,
    pub arguments: Vec<String>,
    pub library_path: Option<String>,
    pub kernel_entry: bool,
}

pub(super) enum Command {
    Exit(i32),
    Run(Program),
}

pub(super) fn environment_value(environment: Option<&[String]>, key: &str) -> Option<String> {
    match environment {
        Some(environment) => environment.iter().find_map(|entry| {
            let (name, value) = entry.split_once('=')?;
            (name == key).then(|| value.to_owned())
        }),
        None => std::env::var(format!("KINAKAZE_GUEST_ENV_{key}")).ok(),
    }
}

#[derive(Default)]
struct Options {
    verify: bool,
    list: bool,
    library_path: Option<String>,
    argv0: Option<String>,
    first: usize,
}

fn options(arguments: &[String]) -> Result<Options, String> {
    let mut options = Options {
        first: 1,
        ..Options::default()
    };
    while let Some(argument) = arguments.get(options.first) {
        match argument.as_str() {
            "--verify" => options.verify = true,
            "--list" => options.list = true,
            // No ld.so.cache is used by the hosted linker.
            "--inhibit-cache" => {}
            "--library-path" | "--argv0" => {
                options.first += 1;
                let value = arguments
                    .get(options.first)
                    .ok_or_else(|| format!("{argument} requires an argument"))?
                    .clone();
                if argument == "--library-path" {
                    options.library_path = Some(value);
                } else {
                    options.argv0 = Some(value);
                }
            }
            "--" => {
                options.first += 1;
                break;
            }
            value if value.starts_with('-') => return Err(format!("unsupported option {value}")),
            _ => break,
        }
        options.first += 1;
    }
    Ok(options)
}

fn write(fd: i32, text: &str) {
    let mut bytes = text.as_bytes();
    while !bytes.is_empty() {
        match kinakaze_vfs::write(fd, bytes) {
            Ok(0) | Err(_) => break,
            Ok(count) => bytes = &bytes[count..],
        }
    }
}

fn verified(bytes: &[u8]) -> i32 {
    let Ok(elf) = kinakaze_elf::ElfFile::parse(bytes) else {
        return 1;
    };
    if !matches!(
        elf.header().object_type,
        kinakaze_elf::ET_EXEC | kinakaze_elf::ET_DYN
    ) || !elf.dynamic_info().is_ok_and(|dynamic| dynamic.is_some())
    {
        return 1;
    }
    match elf.program_headers() {
        Ok(headers)
            if headers
                .iter()
                .any(|header| header.kind == kinakaze_elf::PT_INTERP) =>
        {
            0
        }
        Ok(_) => 2,
        Err(_) => 1,
    }
}

pub(super) fn prepare(
    path: PathBuf,
    image: ImmutableBytes,
    arguments: Vec<String>,
    environment: Option<&[String]>,
) -> Result<Command, LinkError> {
    let registry = super::configuration()?.providers.clone();
    let native = registry.is_interpreter_image(&image);
    let trace = environment_value(environment, "LD_TRACE_LOADED_OBJECTS")
        .is_some_and(|value| !value.is_empty());
    let mut program = Program {
        path,
        image,
        arguments,
        library_path: None,
        kernel_entry: false,
    };
    if !native && !trace {
        return Ok(Command::Run(program));
    }

    // A short loader command still participates in exec's readiness/commit
    // barrier, and writes through the inherited guest descriptor table.
    kinakaze_runtime::authority::activate_image().map_err(|errno| {
        LinkError::Execution(format!("interpreter activation failed: errno {errno}"))
    })?;
    let mut list = trace;
    if native {
        match program.arguments.get(1).map(String::as_str) {
            Some("--version") => {
                write(
                    1,
                    concat!(
                        "Kinakaze ELF dynamic linker ",
                        env!("CARGO_PKG_VERSION"),
                        "\n"
                    ),
                );
                return Ok(Command::Exit(0));
            }
            Some("--help") => {
                write(
                    1,
                    "Usage: ld.so [--verify | --list] [--library-path PATH] [--argv0 NAME] PROGRAM [ARGS...]\n",
                );
                return Ok(Command::Exit(0));
            }
            _ => {}
        }
        let options = match options(&program.arguments) {
            Ok(options) => options,
            Err(error) => {
                write(2, &format!("ld.so: {error}\n"));
                return Ok(Command::Exit(1));
            }
        };
        let Some(target) = program.arguments.get(options.first) else {
            write(2, "ld.so: missing program\n");
            return Ok(Command::Exit(1));
        };
        let target_image = kinakaze_vfs::resolve_linux_path(target)
            .map_err(|error| LinkError::Execution(error.to_string()))
            .and_then(|path| {
                kinakaze_link::snapshot_guest_image(&path)
                    .map(|image| (path, image))
                    .map_err(LinkError::from)
            });
        if options.verify {
            return Ok(Command::Exit(
                target_image.map_or(1, |(_, image)| verified(&image)),
            ));
        }
        let (path, image) = target_image?;
        let mut arguments = program.arguments[options.first..].to_vec();
        if let Some(argv0) = options.argv0 {
            arguments[0] = argv0;
        }
        program = Program {
            path,
            image,
            arguments,
            library_path: options.library_path,
            kernel_entry: true,
        };
        list |= options.list;
    }
    if !list {
        return Ok(Command::Run(program));
    }
    if verified(&program.image) == 1 {
        // Reject scripts and malformed images; static ELF needs no dependencies.
        let elf = kinakaze_elf::ElfFile::parse(&program.image)?;
        if !matches!(
            elf.header().object_type,
            kinakaze_elf::ET_EXEC | kinakaze_elf::ET_DYN
        ) {
            return Err(LinkError::NoEntryPoint {
                object: program.path.display().to_string(),
            });
        }
        if elf.dynamic_info()?.is_some() {
            return Err(LinkError::Execution("invalid dynamic executable".into()));
        }
        write(1, "\tstatically linked\n");
        return Ok(Command::Exit(0));
    }
    let mut linker = Linker::new(
        SearchPaths::with_host_directory(super::configuration()?.root.clone())
            .with_process_namespace(),
    );
    linker.register_provider_registry(registry)?;
    let library_path = program
        .library_path
        .or_else(|| environment_value(environment, "LD_LIBRARY_PATH"));
    linker.set_library_path(library_path.as_deref(), &program.path);
    let root = linker.load_process_graph(&program.path, Some(program.image))?;
    write(1, &linker.dependency_listing(root));
    Ok(Command::Exit(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parsing_stops_at_the_program_and_preserves_its_options() {
        let argv = [
            "ld.so",
            "--library-path",
            ":/opt/lib",
            "--argv0",
            "name",
            "./app",
            "--verify",
        ]
        .map(str::to_owned);
        let parsed = options(&argv).unwrap();
        assert_eq!(parsed.first, 5);
        assert_eq!(parsed.library_path.as_deref(), Some(":/opt/lib"));
        assert_eq!(parsed.argv0.as_deref(), Some("name"));
        assert!(!parsed.verify);
        assert!(options(&["ld.so".into(), "--library-path".into()]).is_err());
        assert!(options(&["ld.so".into(), "--unknown".into()]).is_err());
    }
    #[test]
    fn explicit_empty_environment_never_inherits_host_loader_options() {
        assert_eq!(environment_value(Some(&[]), "LD_LIBRARY_PATH"), None);
        assert_eq!(
            environment_value(Some(&["LD_LIBRARY_PATH=/guest".into()]), "LD_LIBRARY_PATH")
                .as_deref(),
            Some("/guest")
        );
    }
}
