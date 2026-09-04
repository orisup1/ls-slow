# ls-slow

Slowly print a directory tree, one line at a time.

```text
$ sls --depth 2 src
src
└── main.rs
```

## Usage

```text
sls [OPTIONS] [PATH]

Options:
  --depth LEVELS  Levels to display [default: 2]
  --delay-ms MS   Delay after each line [default: 25]
  -h, --help      Print help
```

`PATH` defaults to the current directory. Hidden entries and the contents of
directory symlinks are not shown.

## Add program to PATH

```sh
cargo install --path .
```

Cargo installs `sls` into `~/.cargo/bin`; make sure that directory is in `PATH`.
