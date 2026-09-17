use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirEntry};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;
use std::thread;
use std::time::Duration;

const HELP: &str = "\
Usage: sls [OPTIONS] [PATH]

Slowly print a directory tree. PATH defaults to the current directory.

Options:
  -d, --depth LEVELS  Levels to display; 0 shows only the root [default: 2]
  --delay-ms MS   Delay per line [default: 25 in terminals, 0 otherwise]
  -a, --all       Show hidden entries
  -h, --help      Print help

Hidden entries are omitted by default. Directory symlinks are not followed.
Markers: / directory, @ symlink, [depth limit] contents not inspected.
Use --delay-ms 0 to disable animation. Explicit delays also apply to pipes.
";

struct Config {
    root: PathBuf,
    depth: usize,
    delay: Duration,
    all: bool,
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn parse_number<T: FromStr>(
    args: &mut impl Iterator<Item = OsString>,
    option: &str,
) -> io::Result<T> {
    let value = args
        .next()
        .ok_or_else(|| invalid_input(format!("{option} requires a value")))?;
    value
        .to_str()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| {
            invalid_input(format!(
                "invalid value for {option}: {}",
                value.to_string_lossy()
            ))
        })
}

fn parse_args(
    args: impl IntoIterator<Item = OsString>,
    terminal: bool,
) -> io::Result<Option<Config>> {
    let mut args = args.into_iter();
    let mut root = None;
    let mut depth = 2;
    let mut delay_ms = if terminal { 25 } else { 0 };
    let mut all = false;
    let mut options = true;

    while let Some(arg) = args.next() {
        if options && (arg == OsStr::new("-h") || arg == OsStr::new("--help")) {
            return Ok(None);
        } else if options && (arg == OsStr::new("-a") || arg == OsStr::new("--all")) {
            all = true;
        } else if options && (arg == OsStr::new("-d") || arg == OsStr::new("--depth")) {
            let arg_str = arg.to_string_lossy();
            depth = if let Some(next) = args.next() {
                let next_str = next.to_string_lossy();
                if next_str.starts_with('-') {
                    1
                } else {
                    next
                        .to_str()
                        .and_then(|v| v.parse().ok())
                        .ok_or_else(|| invalid_input(format!("invalid value for {arg_str}: {}", next.to_string_lossy())))?
                }
            } else {
                1
            };
        } else if options && arg == OsStr::new("--delay-ms") {
            delay_ms = parse_number(&mut args, "--delay-ms")?;
        } else if options && arg == OsStr::new("--") {
            options = false;
        } else if options && arg.to_string_lossy().starts_with('-') {
            return Err(invalid_input(format!(
                "unknown option: {}",
                arg.to_string_lossy()
            )));
        } else if root.replace(PathBuf::from(&arg)).is_some() {
            return Err(invalid_input(format!(
                "unexpected argument: {}",
                arg.to_string_lossy()
            )));
        }
    }

    Ok(Some(Config {
        root: root.unwrap_or_else(|| PathBuf::from(".")),
        depth,
        delay: Duration::from_millis(delay_ms),
        all,
    }))
}

fn display_name(name: &OsStr) -> String {
    name.to_string_lossy()
        .chars()
        .flat_map(char::escape_debug)
        .collect()
}

fn path_error(path: &Path, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("{}: {error}", display_name(path.as_os_str())),
    )
}

fn list_dir(path: &Path, all: bool) -> io::Result<Vec<DirEntry>> {
    let entries = fs::read_dir(path).map_err(|error| path_error(path, error))?;
    let mut items = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|error| path_error(path, error))?;
        if all || !entry.file_name().to_string_lossy().starts_with('.') {
            items.push(entry);
        }
    }

    items.sort_by_key(|entry| entry.file_name());
    Ok(items)
}

fn print_line<W: Write>(writer: &mut W, line: &str, delay: Duration) -> io::Result<()> {
    writeln!(writer, "{line}")?;
    writer.flush()?;
    if !delay.is_zero() {
        thread::sleep(delay);
    }
    Ok(())
}

fn walk<W: Write>(
    writer: &mut W,
    dir: &Path,
    prefix: &str,
    depth: usize,
    config: &Config,
) -> io::Result<()> {
    if depth > config.depth {
        return Ok(());
    }

    let items = list_dir(dir, config.all)?;
    let count = items.len();
    for (index, entry) in items.iter().enumerate() {
        let last = index + 1 == count;
        let branch = if last { "└── " } else { "├── " };
        let name = display_name(&entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|error| path_error(&entry.path(), error))?;
        let marker = if kind.is_symlink() {
            "@"
        } else if kind.is_dir() {
            "/"
        } else {
            ""
        };
        let limit = if kind.is_dir() && depth == config.depth {
            " [depth limit]"
        } else {
            ""
        };
        print_line(
            writer,
            &format!("{prefix}{branch}{name}{marker}{limit}"),
            config.delay,
        )?;

        if depth < config.depth && kind.is_dir() {
            let next_prefix = format!("{prefix}{}", if last { "    " } else { "│   " });
            walk(writer, &entry.path(), &next_prefix, depth + 1, config)?;
        }
    }
    Ok(())
}

fn run(
    args: impl IntoIterator<Item = OsString>,
    writer: &mut impl Write,
    terminal: bool,
) -> io::Result<()> {
    let Some(config) = parse_args(args, terminal)? else {
        return writer.write_all(HELP.as_bytes());
    };

    let metadata = fs::metadata(&config.root).map_err(|error| path_error(&config.root, error))?;
    if !metadata.is_dir() {
        return Err(invalid_input(format!(
            "{}: not a directory",
            display_name(config.root.as_os_str())
        )));
    }

    let root_name = display_name(config.root.file_name().unwrap_or(config.root.as_os_str()));
    let marker = if fs::symlink_metadata(&config.root)
        .map_err(|error| path_error(&config.root, error))?
        .file_type()
        .is_symlink()
    {
        "@"
    } else if root_name.ends_with('/') {
        ""
    } else {
        "/"
    };
    let limit = if config.depth == 0 {
        " [depth limit]"
    } else {
        ""
    };
    print_line(writer, &format!("{root_name}{marker}{limit}"), config.delay)?;
    walk(writer, &config.root, "", 1, &config)
}

fn main() -> ExitCode {
    let stdout = io::stdout();
    let mut writer = stdout.lock();

    match run(env::args_os().skip(1), &mut writer, stdout.is_terminal()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sls: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct BrokenWriter;

    impl Write for BrokenWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn display_and_delay() {
        assert_eq!(
            display_name(OsStr::new("line\n\t\r\x1b")),
            r"line\n\t\r\u{1b}"
        );
        assert_eq!(display_name(OsStr::new("café 日本")), "café 日本");
        assert_eq!(display_name(OsStr::new(r"literal\n")), r"literal\\n");
        for terminal in [false, true] {
            let config = parse_args([], terminal).unwrap().unwrap();
            assert_eq!(config.delay.as_millis(), if terminal { 25 } else { 0 });
            for delay in ["0", "7"] {
                let config = parse_args(["--delay-ms", delay].map(OsString::from), terminal)
                    .unwrap()
                    .unwrap();
                assert_eq!(config.delay.as_millis(), delay.parse::<u128>().unwrap());
            }
        }
    }

    #[test]
    fn core_behaviors() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp = env::temp_dir().join(format!("ls-slow-{}-{unique}", std::process::id()));
        let root = temp.join("root");
        let outside = temp.join("outside");
        fs::create_dir_all(root.join("sub/deeper")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        File::create(root.join("visible")).unwrap();
        File::create(root.join(".hidden")).unwrap();
        File::create(root.join("sub/child")).unwrap();
        File::create(root.join("sub/deeper/too-deep")).unwrap();
        File::create(outside.join("outside-only")).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            use std::os::unix::fs::symlink;

            assert_eq!(
                display_name(&OsString::from_vec(b"bad-\xff".to_vec())),
                "bad-�"
            );
            symlink(&outside, root.join("link")).unwrap();
        }

        let mut output = Vec::new();
        run(
            [
                OsString::from("--delay-ms"),
                OsString::from("0"),
                OsString::from("--depth"),
                OsString::from("2"),
                root.as_os_str().to_owned(),
            ],
            &mut output,
            false,
        )
        .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.starts_with("root/\n"));
        assert!(output.contains("sub/\n"));
        assert!(output.contains("deeper/ [depth limit]\n"));
        assert!(output.contains("visible"));
        assert!(output.contains("child"));
        assert!(!output.contains(".hidden"));
        assert!(!output.contains("too-deep"));

        #[cfg(unix)]
        {
            assert!(output.contains("link@\n"));
            assert!(!output.contains("outside-only"));
        }

        let mut output = Vec::new();
        let error = run([temp.join("missing").into_os_string()], &mut output, false).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(output.is_empty());

        let error = run(
            [root.as_os_str().to_owned(), OsString::from("extra")],
            &mut output,
            false,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);

        let error = run(
            [
                OsString::from("--delay-ms"),
                OsString::from("5000"),
                root.as_os_str().to_owned(),
            ],
            &mut BrokenWriter,
            false,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);

        for option in ["-a", "--all"] {
            let mut output = Vec::new();
            run(
                [OsString::from(option), root.clone().into_os_string()],
                &mut output,
                false,
            )
            .unwrap();
            assert!(String::from_utf8(output).unwrap().contains(".hidden"));
        }
        let mut output = Vec::new();
        run(
            [
                OsString::from("--depth"),
                OsString::from("0"),
                root.into_os_string(),
            ],
            &mut output,
            false,
        )
        .unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), "root/ [depth limit]\n");
        fs::remove_dir_all(temp).unwrap();
    }
}
