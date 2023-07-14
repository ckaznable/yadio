# Yadio

Listen youtube streaming with chat in CLI.

## Requirement

- `yt-dlp`

## Installation

### Install from Github Release

Download the last version binary depending on your configuration here: [Release Page](https://github.com/ckaznable/yadio/releases/latest)

Then you just need to enter this command in your terminal:

```shell
tar -xf <downloaded_archive> yadio && sudo mv yadio /usr/local/bin
```

### Install from crates.io

If you're a Rust programmer, yadio can be installed with cargo.

```shell
cargo install yadio
```

## Building

yadio is written in Rust, so you'll need to grab a [Rust installation](https://www.rust-lang.org/) in order to compile it.

```shell
git clone https://github.com/ckaznable/yadio
cd yadio
cargo build --release
```

## Usage

```text
Usage: yadio <URL>

Arguments:
  <URL>  youtube url or youtube video id

Options:
      --chatroom  enable chatroom output
  -h, --help      Print help
  -V, --version   Print version
```

## LICENSE

[MIT](./LICENSE)
