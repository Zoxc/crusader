# Network bandwidth and latency tester

## Setup

Run `cargo build --release` to build the executable which is placed in `target/release`.

## Usage

To host a server run:
```sh
tool serve
```
It uses TCP and UDP port 30481.


To do a test run:
```sh
tool test <server-host>
```
This produces an graph output file named `plot.png`.
