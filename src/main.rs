use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirEntry};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;
use std::thread;
use std::time::Duration;

const HELP: &str = "\
Usage: sls [OPTIONS] [PATH]

Slowly print a directory tree. PATH defaults to the current directory.

Options:
  --depth LEVELS  Levels to display [default: 2]
  --delay-ms MS   Delay after each line [default: 25]
  -h, --help      Print help
";

struct Config {
    root: PathBuf,
    depth: usize,
    delay: Duration,
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

fn parse_args(args: impl IntoIterator<Item = OsString>) -> io::Result<Option<Config>> {
    let mut args = args.into_iter();
    let mut root = None;
    let mut depth = 2;
    let mut delay_ms = 25;
    let mut options = true;

    while let Some(arg) = args.next() {
        if options && (arg == OsStr::new("-h") || arg == OsStr::new("--help")) {
            return Ok(None);
        } else if options && arg == OsStr::new("--depth") {
            depth = parse_number(&mut args, "--depth")?;
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
    }))
}

fn path_error(path: &Path, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

fn list_dir(path: &Path) -> io::Result<Vec<DirEntry>> {
    let entries = fs::read_dir(path).map_err(|error| path_error(path, error))?;
    let mut items = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|error| path_error(path, error))?;
        if !entry.file_name().to_string_lossy().starts_with('.') {
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
    max_depth: usize,
    delay: Duration,
) -> io::Result<()> {
    if depth > max_depth {
        return Ok(());
    }

    let items = list_dir(dir)?;
    let count = items.len();
    for (index, entry) in items.iter().enumerate() {
        let last = index + 1 == count;
        let branch = if last { "└── " } else { "├── " };
        let name = entry.file_name();
        print_line(
            writer,
            &format!("{prefix}{branch}{}", name.to_string_lossy()),
            delay,
        )?;

        if depth < max_depth
            && entry
                .file_type()
                .map_err(|error| path_error(&entry.path(), error))?
                .is_dir()
        {
            let next_prefix = format!("{prefix}{}", if last { "    " } else { "│   " });
            walk(
                writer,
                &entry.path(),
                &next_prefix,
                depth + 1,
                max_depth,
                delay,
            )?;
        }
    }
    Ok(())
}

fn run(args: impl IntoIterator<Item = OsString>, writer: &mut impl Write) -> io::Result<()> {
    let Some(config) = parse_args(args)? else {
        return writer.write_all(HELP.as_bytes());
    };

    let metadata = fs::metadata(&config.root).map_err(|error| path_error(&config.root, error))?;
    if !metadata.is_dir() {
        return Err(invalid_input(format!(
            "{}: not a directory",
            config.root.display()
        )));
    }

    let root_name = config
        .root
        .file_name()
        .unwrap_or(config.root.as_os_str())
        .to_string_lossy();
    print_line(writer, &root_name, config.delay)?;
    walk(writer, &config.root, "", 1, config.depth, config.delay)
}

fn main() -> ExitCode {
    let stdout = io::stdout();
    let mut writer = stdout.lock();

    match run(env::args_os().skip(1), &mut writer) {
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

            File::create(root.join(OsString::from_vec(b"bad-\xff".to_vec()))).unwrap();
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
        )
        .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("visible"));
        assert!(output.contains("child"));
        assert!(!output.contains(".hidden"));
        assert!(!output.contains("too-deep"));

        #[cfg(unix)]
        {
            assert!(output.contains("bad-�"));
            assert!(!output.contains("outside-only"));
        }

        let mut output = Vec::new();
        let error = run([temp.join("missing").into_os_string()], &mut output).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(output.is_empty());

        let error = run(
            [root.as_os_str().to_owned(), OsString::from("extra")],
            &mut output,
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
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);

        fs::remove_dir_all(temp).unwrap();
    }
}
