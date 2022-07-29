# Crusader Network Tester

## Setup

Run `cargo build --release` in the `src` folder to build the executables which are placed in `src/target/release`.

## Command line usage

To host a server run:
```sh
crusader serve
```
It uses TCP and UDP port 30481.


To do a test run:
```sh
crusader test <server-host>
```
This produces an graph output file named `plot.png`.


## Graphical interface

There is also a binary with a graphical interface allowing you to use a client, a server and interact with results.

<img src="media/gui.png">