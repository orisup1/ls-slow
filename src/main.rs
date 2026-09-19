use std::cmp::Reverse;
use std::collections::HashMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirEntry};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::str::FromStr;
use std::thread;
use std::time::Duration;

const HELP: &str = "\
Usage: sls [OPTIONS] [PATH]

Slowly print a directory tree. PATH defaults to the current directory.

Options:
  -d, --depth LEVELS  Levels to display; 0 shows only the root [default: 2]
  --delay-ms MS   Delay per line [default: 25 in terminals, 0 otherwise]
  --color WHEN   Color output: auto, always, never [default: auto]
  --ignore LIST  Comma-separated name patterns (* and ?); repeatable
  --size         Show file sizes (decimal units)
  --sort KEY     Sort by name, size (largest), or modified (newest)
  --git          Show Git status
  --summary      Count displayed directories and files
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
    color: bool,
    ignore: Vec<String>,
    size: bool,
    sort: String,
    git: bool,
    summary: bool,
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
    let mut args = args.into_iter().peekable();
    let mut root = None;
    let mut depth = 2;
    let mut delay_ms = if terminal { 25 } else { 0 };
    let mut all = false;
    let auto_color = terminal
        && env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
        && env::var_os("TERM").is_none_or(|value| value != "dumb");
    let mut color = auto_color;
    let mut options = true;
    let mut ignore = Vec::new();
    let mut size = false;
    let mut sort = String::from("name");
    let mut git = false;
    let mut summary = false;

    while let Some(arg) = args.next() {
        if options && (arg == OsStr::new("-h") || arg == OsStr::new("--help")) {
            return Ok(None);
        } else if options && (arg == OsStr::new("-a") || arg == OsStr::new("--all")) {
            all = true;
        } else if options && (arg == OsStr::new("-d") || arg == OsStr::new("--depth")) {
            depth = if args
                .peek()
                .is_none_or(|next| next.to_string_lossy().starts_with('-'))
            {
                1
            } else {
                parse_number(&mut args, "--depth")?
            };
        } else if options && arg == OsStr::new("--delay-ms") {
            delay_ms = parse_number(&mut args, "--delay-ms")?;
        } else if options && arg == OsStr::new("--color") {
            color = match args.next().as_deref().and_then(OsStr::to_str) {
                Some("auto") => auto_color,
                Some("always") => true,
                Some("never") => false,
                _ => return Err(invalid_input("--color requires auto, always, or never")),
            };
        } else if options && arg == OsStr::new("--ignore") {
            let value = args
                .next()
                .ok_or_else(|| invalid_input("--ignore requires patterns"))?;
            let value = value
                .to_str()
                .ok_or_else(|| invalid_input("--ignore requires UTF-8 patterns"))?;
            if value.split(',').any(str::is_empty) {
                return Err(invalid_input("--ignore patterns must not be empty"));
            }
            ignore.extend(value.split(',').map(String::from));
        } else if options && arg == OsStr::new("--size") {
            size = true;
        } else if options && arg == OsStr::new("--sort") {
            sort = match args.next().as_deref().and_then(OsStr::to_str) {
                Some(value @ ("name" | "size" | "modified")) => value.to_owned(),
                _ => return Err(invalid_input("--sort requires name, size, or modified")),
            };
        } else if options && arg == OsStr::new("--git") {
            git = true;
        } else if options && arg == OsStr::new("--summary") {
            summary = true;
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
        color,
        ignore,
        size,
        sort,
        git,
        summary,
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

fn colored_name(path: &Path, name: &str, marker: &str, color: bool) -> io::Result<String> {
    let label = format!("{name}{marker}");
    if !color {
        return Ok(label);
    }
    let style = if marker == "@" {
        "36"
    } else if marker == "/" || name.ends_with('/') {
        "1;34"
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::{FileTypeExt, PermissionsExt};
            let metadata = fs::symlink_metadata(path).map_err(|error| path_error(path, error))?;
            let kind = metadata.file_type();
            if kind.is_socket() {
                return Ok(format!("\x1b[35m{label}\x1b[0m"));
            }
            if kind.is_fifo() || kind.is_block_device() || kind.is_char_device() {
                return Ok(format!("\x1b[33m{label}\x1b[0m"));
            }
            if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
                return Ok(format!("\x1b[1;32m{label}\x1b[0m"));
            }
        }
        match path
            .extension()
            .and_then(OsStr::to_str)
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "zip" | "gz" | "bz2" | "xz" | "zst" | "tar" | "7z" | "rar" => "1;31",
            "jpg" | "jpeg" | "png" | "gif" | "svg" | "webp" | "ico" | "mp4" | "mkv" | "mov"
            | "webm" => "35",
            "mp3" | "wav" | "flac" | "ogg" | "m4a" => "36",
            "md" | "txt" | "pdf" | "doc" | "docx" | "rst" => "33",
            "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "go" | "c" | "h" | "cpp" | "java"
            | "sh" => "32",
            _ => return Ok(label),
        }
    };
    Ok(format!("\x1b[{style}m{label}\x1b[0m"))
}

fn matches_pattern(pattern: &str, name: &str) -> bool {
    let name: Vec<char> = name.chars().collect();
    let mut matched = vec![false; name.len() + 1];
    matched[0] = true;
    for token in pattern.chars() {
        if token == '*' {
            for i in 1..matched.len() {
                matched[i] |= matched[i - 1];
            }
        } else {
            for i in (1..matched.len()).rev() {
                matched[i] = matched[i - 1] && (token == '?' || token == name[i - 1]);
            }
            matched[0] = false;
        }
    }
    matched[name.len()]
}

fn human_size(bytes: u64) -> String {
    let mut value = bytes as f64;
    for unit in ["B", "KB", "MB", "GB", "TB", "PB", "EB"] {
        if value < 1000.0 || unit == "EB" {
            return if unit == "B" {
                format!("{bytes} B")
            } else {
                format!("{value:.1} {unit}")
            };
        }
        value /= 1000.0;
    }
    unreachable!()
}

fn git_path(bytes: &[u8]) -> io::Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(PathBuf::from(OsStr::from_bytes(bytes)))
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(bytes)
            .map(PathBuf::from)
            .map_err(|_| invalid_input("Git returned a non-UTF-8 path"))
    }
}

fn git_status(root: &Path) -> io::Result<HashMap<PathBuf, String>> {
    let git = |args: &[&str]| -> io::Result<Vec<u8>> {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .map_err(|error| io::Error::new(error.kind(), format!("--git: {error}")))?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "--git: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(output.stdout)
    };
    let top = git(&["rev-parse", "--show-toplevel"])?;
    let top = git_path(top.strip_suffix(b"\n").unwrap_or(&top))?;
    let output = git(&[
        "status",
        "--porcelain=v1",
        "-z",
        "--no-renames",
        "--ignored",
        "--untracked-files=all",
        "--",
        ".",
    ])?;
    let mut statuses = HashMap::new();
    for record in output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        if record.len() < 4 || record[2] != b' ' {
            return Err(io::Error::other("--git: invalid status record"));
        }
        let path = top.join(git_path(&record[3..])?);
        let status = String::from_utf8_lossy(&record[..2]).into_owned();
        statuses.insert(path.clone(), status);
        for parent in path
            .ancestors()
            .skip(1)
            .take_while(|parent| parent.starts_with(root) && *parent != root)
        {
            statuses
                .entry(parent.to_owned())
                .or_insert_with(|| "**".to_owned());
        }
    }
    Ok(statuses)
}

fn list_dir(path: &Path, config: &Config) -> io::Result<Vec<DirEntry>> {
    let entries = fs::read_dir(path).map_err(|error| path_error(path, error))?;
    let mut items = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|error| path_error(path, error))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if (config.all || !name.starts_with('.'))
            && !config
                .ignore
                .iter()
                .any(|pattern| matches_pattern(pattern, &name))
        {
            items.push(entry);
        }
    }

    match config.sort.as_str() {
        "size" | "modified" => {
            let mut keyed = Vec::with_capacity(items.len());
            for entry in items {
                let metadata = fs::symlink_metadata(entry.path())
                    .map_err(|error| path_error(&entry.path(), error))?;
                let modified = if config.sort == "modified" {
                    Some(
                        metadata
                            .modified()
                            .map_err(|error| path_error(&entry.path(), error))?,
                    )
                } else {
                    None
                };
                let size = if config.sort == "size" && !metadata.is_dir() {
                    metadata.len()
                } else {
                    0
                };
                keyed.push((Reverse(modified), Reverse(size), entry.file_name(), entry));
            }
            keyed.sort_by(|a, b| (&a.0, &a.1, &a.2).cmp(&(&b.0, &b.1, &b.2)));
            items = keyed.into_iter().map(|(_, _, _, entry)| entry).collect();
        }
        _ => items.sort_by_key(|entry| entry.file_name()),
    }
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
    git: &HashMap<PathBuf, String>,
) -> io::Result<(usize, usize)> {
    if depth > config.depth {
        return Ok((0, 0));
    }

    let mut directories = 0;
    let mut files = 0;
    let items = list_dir(dir, config)?;
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
        if kind.is_dir() {
            directories += 1;
        } else {
            files += 1;
        }
        let size = if config.size && !kind.is_dir() {
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| path_error(&entry.path(), error))?;
            format!(" [{}]", human_size(metadata.len()))
        } else {
            String::new()
        };
        let status = git
            .get(&entry.path())
            .map(|status| format!(" [{status}]"))
            .unwrap_or_default();
        let name = colored_name(&entry.path(), &name, marker, config.color)?;
        print_line(
            writer,
            &format!("{prefix}{branch}{name}{size}{status}{limit}"),
            config.delay,
        )?;

        if depth < config.depth && kind.is_dir() {
            let next_prefix = format!("{prefix}{}", if last { "    " } else { "│   " });
            let (child_dirs, child_files) =
                walk(writer, &entry.path(), &next_prefix, depth + 1, config, git)?;
            directories += child_dirs;
            files += child_files;
        }
    }
    Ok((directories, files))
}

fn run(
    args: impl IntoIterator<Item = OsString>,
    writer: &mut impl Write,
    terminal: bool,
) -> io::Result<()> {
    let Some(mut config) = parse_args(args, terminal)? else {
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
    let root_name = colored_name(&config.root, &root_name, marker, config.color)?;
    let git = if config.git {
        config.root =
            fs::canonicalize(&config.root).map_err(|error| path_error(&config.root, error))?;
        git_status(&config.root)?
    } else {
        HashMap::new()
    };
    print_line(writer, &format!("{root_name}{limit}"), config.delay)?;
    let (directories, files) = walk(writer, &config.root, "", 1, &config, &git)?;
    if config.summary {
        print_line(
            writer,
            &format!("\n{directories} directories, {files} files (displayed; root excluded)"),
            config.delay,
        )?;
    }
    Ok(())
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
    fn listing_options() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("sls-options-{}-{unique}", std::process::id()));
        fs::create_dir_all(root.join("node_modules/nested")).unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("small.txt"), b"hi").unwrap();
        fs::write(root.join("large.bin"), vec![0; 1500]).unwrap();
        fs::write(root.join("sub/child"), b"child").unwrap();
        fs::write(root.join("skip.log"), b"log").unwrap();
        for (name, seconds) in [("small.txt", 200), ("large.bin", 100)] {
            File::options()
                .write(true)
                .open(root.join(name))
                .unwrap()
                .set_times(
                    fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds)),
                )
                .unwrap();
        }
        let render = |options: &[&str], path: &Path| {
            let mut output = Vec::new();
            run(
                options
                    .iter()
                    .map(OsString::from)
                    .chain([path.as_os_str().to_owned()]),
                &mut output,
                false,
            )
            .unwrap();
            String::from_utf8(output).unwrap()
        };
        let output = render(
            &["--ignore", "node_modules,*.log", "--size", "--summary"],
            &root,
        );
        assert!(!output.contains("node_modules"));
        assert!(!output.contains("skip.log"));
        assert!(output.contains("large.bin [1.5 KB]"));
        assert!(output.contains("small.txt [2 B]"));
        assert!(output.contains("1 directories, 3 files (displayed; root excluded)"));
        let output = render(&["--sort", "size"], &root);
        assert!(output.find("large.bin").unwrap() < output.find("small.txt").unwrap());
        let output = render(&["--sort", "modified"], &root);
        assert!(output.find("small.txt").unwrap() < output.find("large.bin").unwrap());
        let output = render(&["--sort", "name"], &root);
        assert!(output.find("large.bin").unwrap() < output.find("node_modules").unwrap());
        let output = render(
            &[
                "-d",
                "--summary",
                "--ignore",
                "node_modules",
                "--ignore",
                "*.log",
            ],
            &root,
        );
        assert!(output.contains("1 directories, 2 files"));
        assert!(render(&["--depth", "0", "--summary"], &root).contains("0 directories, 0 files"));
        assert!(!render(&[], &root).contains("(displayed; root excluded)"));
        for (pattern, name, expected) in [
            ("*.log", "a.log", true),
            ("a?c", "aéc", true),
            ("a*b*c", "abbc", true),
            ("*.log", "a.log.rs", false),
            ("a?", "a", false),
            ("*", "", true),
        ] {
            assert_eq!(matches_pattern(pattern, name), expected);
        }
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1_000_000), "1.0 MB");
        for args in [
            vec!["--sort"],
            vec!["--sort", "bad"],
            vec!["--ignore"],
            vec!["--ignore", "a,,b"],
        ] {
            assert!(parse_args(args.into_iter().map(OsString::from), false).is_err());
        }
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "--quiet"]);
        fs::write(root.join(".gitignore"), "*.log\nnode_modules/\n").unwrap();
        git(&["add", "small.txt", "sub/child"]);
        fs::write(root.join("small.txt"), b"modified").unwrap();
        fs::remove_file(root.join("sub/child")).unwrap();
        fs::write(root.join("sub/new\nfile"), b"new").unwrap();
        let output = render(&["--git"], &root);
        assert!(output.contains("small.txt [AM]"), "{output}");
        assert!(output.contains("large.bin [??]"));
        assert!(output.contains("skip.log [!!]"));
        assert!(output.contains("sub/ [**]"));
        assert!(output.contains("new\\nfile [??]"));
        assert!(render(&["--git"], &root.join("sub")).contains("new\\nfile [??]"));
        assert!(!render(&[], &root).contains("[??]"));
        fs::remove_dir_all(root).unwrap();
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
        assert!(!output.contains('\x1b'));

        File::create(root.join("photo.PNG")).unwrap();
        File::create(root.join("archive.zip")).unwrap();
        File::create(root.join("readme.md")).unwrap();
        File::create(root.join("main.rs")).unwrap();
        File::create(root.join("song.mp3")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            File::create(root.join("launch")).unwrap();
            fs::set_permissions(root.join("launch"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        for mode in ["always", "never", "auto"] {
            let mut colored = Vec::new();
            run(
                ["--delay-ms", "0", "--color", mode]
                    .map(OsString::from)
                    .into_iter()
                    .chain([root.clone().into_os_string()]),
                &mut colored,
                false,
            )
            .unwrap();
            let colored = String::from_utf8(colored).unwrap();
            if mode == "always" {
                for label in [
                    "1;34mroot/",
                    "1;34msub/",
                    "35mphoto.PNG",
                    "1;31marchive.zip",
                    "33mreadme.md",
                    "32mmain.rs",
                    "36msong.mp3",
                ] {
                    assert!(colored.contains(&format!("\x1b[{label}\x1b[0m")));
                }
                assert!(colored.contains("\x1b[0m [depth limit]"));
                #[cfg(unix)]
                {
                    assert!(colored.contains("\x1b[36mlink@\x1b[0m"));
                    assert!(colored.contains("\x1b[1;32mlaunch\x1b[0m"));
                }
            } else {
                assert!(!colored.contains('\x1b'));
            }
        }
        for args in [vec!["--color"], vec!["--color", "invalid"]] {
            assert!(parse_args(args.into_iter().map(OsString::from), false).is_err());
        }

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
