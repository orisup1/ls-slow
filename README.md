# ls-slow

Slowly print a directory tree, one line at a time.

```text
$ sls --depth 2 src
src/
└── main.rs
```

## Usage

```text
sls [OPTIONS] [PATH]

Options:
  --depth LEVELS  Levels to display; 0 shows only the root [default: 2]
  --delay-ms MS   Delay per line [default: 25 in terminals, 0 otherwise]
  --color WHEN   Color output: auto, always, never [default: auto]
  --ignore LIST  Comma-separated name patterns (* and ?); repeatable
  --size         Show file sizes (decimal units)
  --sort KEY     Sort by name, size (largest), or modified (newest)
  --git          Show Git status
  --summary      Count displayed directories and files
  -a, --all       Show hidden entries
  -h, --help      Print help
```

`PATH` defaults to the current directory. Hidden entries and the contents of
directory symlinks are not shown. Use `-a` or `--all` to include hidden entries.

Directories end with `/` and symlinks with `@`. `[depth limit]` means a
directory's contents were not inspected, even if it might be empty.
Control characters in names are escaped (for example, `\n` and `\t`),
and literal backslashes are doubled.

Piped or redirected output has no delay by default. An explicit `--delay-ms`
always applies; use `--delay-ms 0` for instant output in a terminal.

Terminal output colors directories blue, symlinks cyan, Unix executables bold
green, source files green, archives red, images and videos magenta, audio cyan,
and documents yellow. Unix sockets are magenta; pipes and devices are yellow.
Other files keep the terminal's default color. Tree branches and depth markers
stay uncolored.

Colors are disabled for pipes, redirected output, `TERM=dumb`, or a nonempty
`NO_COLOR`. Use `--color always` to force colors or `--color never` to disable them.

```sh
sls --ignore 'node_modules,target,*.log' --size --sort modified
sls --git --summary
```

Ignore patterns match entry names at every depth, including with `--all`.
Matching directories are skipped entirely. Quote wildcard patterns to prevent
shell expansion; `*` matches any characters and `?` matches one character.
Repeat `--ignore` to add more patterns. Paths and character classes are not supported.

`--size` shows bytes or decimal KB/MB/GB for files and symlinks (the link itself,
not its target). Directory sizes are not calculated. Sorting defaults to name;
size sorts largest first (directories count as zero), modified sorts newest first,
and ties sort by name. Sorting applies separately within each directory.

`--git` runs Git once for status and requires Git and a working tree. It shows
Git's two-column status: the first column describes staged changes, the second
unstaged changes; `??` means untracked, `!!` ignored, and `**` marks a directory
containing changes. Renames appear as deletions and additions. Deleted entries
are absent from the tree but still mark their parent directory. Hidden entries
remain hidden unless `--all` is used. Git is not invoked without `--git`.

`--summary` counts only displayed entries, excluding the root and anything
hidden, ignored, or beyond the depth limit. Symlinks and special entries count
as files. Git status and summary are both disabled by default.

## Add program to PATH

```sh
cargo install --path .
```

Cargo installs `sls` into `~/.cargo/bin`; make sure that directory is in `PATH`.
