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

## Add program to PATH

```sh
cargo install --path .
```

Cargo installs `sls` into `~/.cargo/bin`; make sure that directory is in `PATH`.
